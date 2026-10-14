//! Decryption and handling of incoming envelopes.

use std::rc::Rc;

use libsignal_protocol::{
    group_decrypt, message_decrypt_prekey, message_decrypt_signal, process_sender_key_distribution_message,
    DeviceId, PreKeySignalMessage, ProtocolAddress, SenderKeyDistributionMessage, SignalMessage,
    SignalProtocolError,
};
use rusqlite::{params, OptionalExtension};

use super::messages::NewMessage;
use super::send::Job;
use super::Inner;
use crate::api::Envelope;
use crate::content::{Content, DataMessage, ReceiptKind, SyncMessage};
use crate::crypto::unb64;
use crate::error::{CoreError, Result};
use crate::model::{SharedContact, TypingEventPayload};
use crate::util::now_ms;

/// Images and voice notes up to this size are downloaded automatically (accepted
/// conversations only, see engine/requests.rs).
pub(super) const AUTO_DOWNLOAD_MAX: i64 = 16 * 1024 * 1024;
/// Delete-for-everyone is accepted for messages up to this age.
const DELETE_WINDOW_MS: i64 = 2 * 24 * 3600 * 1000 + 3600 * 1000;
/// Edits are accepted for messages up to this age.
const EDIT_WINDOW_MS: i64 = 24 * 3600 * 1000;

impl Inner {
    pub(super) async fn handle_envelope(self: Rc<Self>, mut env: Envelope) -> Result<()> {
        let s = self.session()?;
        let me = s.account.account_id.clone();
        let mut bytes = unb64(&env.content)?;
        if env.kind == super::sealed::SEALED_ENVELOPE {
            // Only the outer layer here; the inner message is decrypted below as usual.
            match self.unseal(&mut env, &bytes).await {
                Ok(inner) => bytes = inner,
                Err(e) => {
                    log::warn!("dropping sealed message: {e}");
                    return Ok(());
                }
            }
        }
        if env.source_account != me && self.is_blocked(&env.source_account) {
            return Ok(());
        }
        let device = DeviceId::new(env.source_device as u8).map_err(|_| CoreError::internal("bad source device"))?;
        let sender = ProtocolAddress::new(env.source_account.clone(), device);
        let local = s.local_address();

        let result: std::result::Result<Vec<u8>, SignalProtocolError> = match env.kind {
            3 => match PreKeySignalMessage::try_from(&bytes[..]) {
                Ok(m) => {
                    let mut st1 = self.signal_store()?;
                    let mut st2 = self.signal_store()?;
                    let mut st3 = self.signal_store()?;
                    let st4 = self.signal_store()?;
                    let mut st5 = self.signal_store()?;
                    message_decrypt_prekey(
                        &m,
                        &sender,
                        &local,
                        &mut st1,
                        &mut st2,
                        &mut st3,
                        &st4,
                        &mut st5,
                        &mut rand::rng(),
                    )
                    .await
                }
                Err(e) => Err(e),
            },
            2 => match SignalMessage::try_from(&bytes[..]) {
                Ok(m) => {
                    let mut st1 = self.signal_store()?;
                    let mut st2 = self.signal_store()?;
                    message_decrypt_signal(&m, &sender, &local, &mut st1, &mut st2, &mut rand::rng()).await
                }
                Err(e) => Err(e),
            },
            7 => {
                let mut st = self.signal_store()?;
                group_decrypt(&bytes, &mut st, &sender).await
            }
            other => {
                log::warn!("ignoring envelope type {other}");
                return Ok(());
            }
        };
        self.process_identity_conflicts()?;

        let plaintext = match result {
            Ok(p) => {
                if env.source_account != me {
                    // They have a device again: send what waited for one.
                    self.wake_no_device_jobs(Some(&env.source_account));
                }
                p
            }
            Err(SignalProtocolError::DuplicatedMessage(..)) => return Ok(()),
            Err(SignalProtocolError::UntrustedIdentity(addr)) => {
                // A device with a foreign identity key: drop it, never ask it to resend.
                log::error!("dropping message from {addr}: identity key does not match the account");
                return Ok(());
            }
            Err(e) => {
                log::warn!("cannot decrypt message from {sender} (type {}): {e}", env.kind);
                self.request_resend(&env)?;
                self.note_undecryptable(&env)?;
                return Ok(());
            }
        };
        let content = match Content::decode(&plaintext) {
            Ok(c) => c,
            Err(e) => {
                log::warn!("undecodable content from {sender}: {e}");
                return Ok(());
            }
        };
        let from_me = env.source_account == me;
        match content {
            Content::Data(dm) => {
                if let Some(head) = dm.kt.clone() {
                    self.kt_on_gossip(&env.source_account, head);
                }
                if env.kind == 7 && dm.group_id != env.group_id {
                    return Err(CoreError::internal("group id mismatch"));
                }
                if from_me {
                    // Our own message, received as a group member on another device.
                    if dm.group_id.is_some() {
                        self.handle_data(&me, env.source_device, dm, true).await?;
                    }
                } else {
                    self.handle_data(&env.source_account, env.source_device, dm, false).await?;
                }
            }
            Content::Receipt { kind, ids } => {
                if !from_me {
                    self.apply_receipts(&env.source_account, kind, &ids, env.timestamp)?;
                }
            }
            Content::Typing { typing, group_id } => {
                if !from_me {
                    self.handle_typing(&env.source_account, typing, group_id)?;
                }
            }
            Content::Sync(sync) => {
                if from_me {
                    self.handle_sync(sync).await?;
                }
            }
            Content::SenderKey { group_id, skdm } => {
                // Only from members an admin confirmed: a server-added account must not be
                // able to post into the group.
                if !self.is_attested_member(&group_id, &env.source_account) {
                    // Kept aside: it is used if an admin confirms the sender later (the
                    // confirmation can arrive after the key).
                    log::warn!("holding sender key for {group_id} from unconfirmed member {}", env.source_account);
                    self.hold_sender_key(&group_id, &env.source_account, env.source_device, &skdm)?;
                    return Ok(());
                }
                let msg = SenderKeyDistributionMessage::try_from(&unb64(&skdm)?[..])?;
                let mut st = self.signal_store()?;
                process_sender_key_distribution_message(&sender, &msg, &mut st).await?;
                log::debug!("sender key from {sender} for group {group_id}");
            }
            Content::Group(ctrl) => {
                self.handle_group_control(&env.source_account, ctrl).await?;
            }
            Content::ProfileUpdated { profile_key } => {
                if !from_me {
                    self.update_profile_key(&env.source_account, &profile_key).await;
                    let me = self.clone();
                    let who = env.source_account.clone();
                    tokio::task::spawn_local(async move {
                        if let Err(e) = me.fetch_profile(&who).await {
                            log::warn!("profile {who}: {e}");
                        }
                    });
                }
            }
            Content::ConversationWiped { before, peer } => {
                if !from_me {
                    self.apply_conversation_wiped(&env.source_account, before)?;
                } else if let Some(peer) = peer.filter(|p| crate::util::is_id(p)) {
                    // Deleted "for everyone" on another of our devices: here too.
                    self.apply_conversation_wiped(&peer, before)?;
                }
            }
            Content::GroupMessagesWiped { group_id, before } => {
                if env.kind == 7 && env.group_id.as_deref() == Some(group_id.as_str()) && self.is_attested_member(&group_id, &env.source_account) {
                    self.apply_group_messages_wiped(&group_id, &env.source_account, before)?;
                }
            }
            Content::GroupHistoryCleared { group_id, before } => {
                if env.kind == 7 && env.group_id.as_deref() == Some(group_id.as_str()) {
                    self.on_group_history_cleared(&group_id, &env.source_account, before)?;
                }
            }
            Content::Call(signal) if signal.group_id.is_some() => {
                let group = if env.kind == 7 { env.group_id.clone() } else { None };
                self.handle_group_call_signal(&env.source_account, env.source_device, env.server_timestamp, group.as_deref(), signal)
                    .await?;
            }
            Content::Call(signal) => {
                // Age is judged by the server's receive time only: the envelope timestamp is
                // chosen by the sender and would let an old offer ring again.
                self.handle_call_signal(&env.source_account, env.source_device, env.server_timestamp, signal)
                    .await?;
            }
            Content::ResendRequest { timestamp, group_id } => {
                self.handle_resend_request(&env.source_account, env.source_device, timestamp, group_id).await?;
            }
        }
        Ok(())
    }

    /// Asks the sender to resend a message we failed to decrypt (at most once per envelope).
    fn request_resend(&self, env: &Envelope) -> Result<()> {
        let key = format!("resend:{}:{}:{}", env.source_account, env.source_device, env.timestamp);
        if self.db.borrow().kv_get(&key)?.is_some() {
            return Ok(());
        }
        self.db.borrow().kv_set(&key, b"1")?;
        if env.kind != 7 {
            // The pairwise session is broken: start a fresh one on our side.
            self.delete_session(&env.source_account, env.source_device)?;
        }
        let content = Content::ResendRequest { timestamp: env.timestamp, group_id: env.group_id.clone() };
        self.enqueue(&Job::Account { account_id: env.source_account.clone(), content })
    }

    /// Answers a resend request: at most 5 times per message, for messages up to 2 days old,
    /// and only to the requester (in a group: pairwise to them, never to the whole group).
    async fn handle_resend_request(&self, requester: &str, device: u32, timestamp: i64, group_id: Option<String>) -> Result<()> {
        if group_id.as_deref().is_some_and(|g| !self.is_attested_member(g, requester)) {
            return Ok(());
        }
        let conv_id = match &group_id {
            Some(g) => self.conversation_id_for_group(g)?,
            None => self.conversation_id_for_peer(requester)?,
        };
        let Some(conv_id) = conv_id else { return Ok(()) };
        let me = self.me()?;
        let id: Option<String> = self
            .db
            .borrow()
            .conn
            .query_row(
                "SELECT id FROM messages WHERE conversation_id = ?1 AND sender_account_id = ?2 AND sent_at = ?3 AND deleted = 0",
                params![conv_id, me, timestamp],
                |r| r.get(0),
            )
            .optional()?;
        if group_id.is_some() {
            // Make sure the requester gets our sender key again.
            self.delete_sender_key_shared_for(requester, device)?;
        }
        let Some(id) = id else { return Ok(()) };
        // Still in our outbox (or failed): it goes out with the next send attempt anyway.
        if self.message_row(&id)?.is_none_or(|m| m.status == "pending" || m.status == "failed") {
            return Ok(());
        }
        if !self.allow_resend(&id, timestamp)? {
            log::warn!("ignoring resend request from {requester} for {id}: limit reached or too old");
            return Ok(());
        }
        match group_id {
            Some(g) => {
                // Attachments are already uploaded (sent message): nothing to wait for.
                let Some((_, mut dm)) = self.build_data_message(&id).await? else { return Ok(()) };
                dm.group_id = Some(g);
                self.enqueue(&Job::Account { account_id: requester.to_string(), content: Content::Data(dm) })?;
            }
            None => self.enqueue(&Job::Message { message_id: id })?,
        }
        Ok(())
    }

    fn delete_sender_key_shared_for(&self, account_id: &str, device_id: u32) -> Result<()> {
        self.db.borrow().conn.execute(
            "DELETE FROM sender_key_shared WHERE account_id = ?1 AND device_id = ?2",
            params![account_id, device_id],
        )?;
        Ok(())
    }

    fn handle_typing(&self, sender: &str, typing: bool, group_id: Option<String>) -> Result<()> {
        if group_id.as_deref().is_some_and(|g| !self.is_attested_member(g, sender)) {
            return Ok(());
        }
        let conv = match &group_id {
            Some(g) => self.conversation_id_for_group(g)?,
            None => self.conversation_id_for_peer(sender)?,
        };
        if let Some(conversation_id) = conv {
            let sender_name = self.member_name(&conversation_id, sender);
            self.emit(
                "typing",
                &TypingEventPayload {
                    conversation_id,
                    account_id: sender.to_string(),
                    sender_name,
                    typing,
                },
            );
        }
        Ok(())
    }

    async fn handle_sync(self: &Rc<Self>, sync: SyncMessage) -> Result<()> {
        match sync {
            SyncMessage::Sent { destination, message } => {
                // Written from another of our devices: that accepts the conversation.
                let conv = self.ensure_direct_conversation(&destination, true).await?;
                if self.mark_accepted(&conv)? {
                    self.download_held_attachments(&conv)?;
                }
                let me = self.me()?;
                self.handle_data_in_direct(&me, 0, &destination, message, true).await
            }
            SyncMessage::Read { entries } => {
                let mut touched = std::collections::HashSet::new();
                for (sender, id) in entries {
                    let conv: Option<String> = self
                        .db
                        .borrow()
                        .conn
                        .query_row(
                            "SELECT conversation_id FROM messages WHERE id = ?1 AND sender_account_id = ?2",
                            params![id, sender],
                            |r| r.get(0),
                        )
                        .optional()?;
                    if let Some(c) = conv {
                        self.db.borrow().conn.execute("UPDATE messages SET read_local = 1 WHERE id = ?1", [&id])?;
                        self.start_expiry_timers("id = ?1", &id)?;
                        touched.insert(c);
                    }
                }
                for c in touched {
                    self.recount_unread(&c)?;
                    self.emit_conversation(&c);
                }
                Ok(())
            }
            SyncMessage::VaultUpdated => {
                self.vault_notify.notify_one();
                Ok(())
            }
            SyncMessage::ProfileUpdated => {
                let me = self.me()?;
                self.fetch_profile(&me).await
            }
            SyncMessage::ViewOnceOpened { sender, id } => self.apply_view_once_opened_sync(&sender, &id),
            SyncMessage::ProfileKey { key } => self.apply_profile_key_sync(&key).await,
        }
    }

    /// Handles a data message from `sender` (another account, or ourselves via a group).
    async fn handle_data(self: &Rc<Self>, sender: &str, device: u32, dm: DataMessage, outgoing: bool) -> Result<()> {
        // Sender-chosen ids become file names (attachments) and row keys: UUIDs only.
        let targets_ok = dm.quote.as_deref().is_none_or(crate::util::is_id)
            && dm.delete.as_deref().is_none_or(crate::util::is_id)
            && dm.reaction.as_ref().is_none_or(|r| crate::util::is_id(&r.target_id))
            && dm.edit.as_ref().is_none_or(|e| crate::util::is_id(&e.target_id));
        if !crate::util::is_id(&dm.id) || !targets_ok {
            log::warn!("dropping message from {sender} with a malformed id");
            return Ok(());
        }
        if let Some(pk) = &dm.profile_key {
            if !outgoing {
                self.update_profile_key(sender, pk).await;
                self.refresh_profile_if_stale(sender);
            }
        }
        match dm.group_id.clone() {
            Some(group_id) => {
                // Group content only from attested members (also blocks removed members and
                // strangers sending pairwise messages tagged with the group id).
                if !outgoing && self.attested(&group_id).is_some() && !self.is_attested_member(&group_id, sender) {
                    log::warn!("dropping group message for {group_id} from non-member {sender}");
                    return Ok(());
                }
                let conv_id = match self.conversation_id_for_group(&group_id)? {
                    Some(c) => c,
                    None => {
                        // Unknown group (we missed the invitation): fetch it.
                        let _ = self.sync_group(&group_id, None).await;
                        match self.conversation_id_for_group(&group_id)? {
                            Some(c) => c,
                            None => return Ok(()),
                        }
                    }
                };
                self.apply_data(&conv_id, sender, device, dm, outgoing)
            }
            None => self.handle_data_in_direct(sender, device, sender, dm, outgoing).await,
        }
    }

    async fn handle_data_in_direct(
        self: &Rc<Self>,
        sender: &str,
        device: u32,
        peer: &str,
        dm: DataMessage,
        outgoing: bool,
    ) -> Result<()> {
        let conv_id = self.ensure_direct_conversation(peer, outgoing).await?;
        self.apply_data(&conv_id, sender, device, dm, outgoing)
    }

    /// Applies a data message to a conversation (new message or modification).
    fn apply_data(self: &Rc<Self>, conv_id: &str, sender: &str, device: u32, mut dm: DataMessage, outgoing: bool) -> Result<()> {
        let now = now_ms();
        // Senders choose `sent_at`; never let it lie far in the future (list ordering).
        dm.sent_at = dm.sent_at.min(now + 5 * 60_000);
        if !limit_incoming(&mut dm) {
            log::warn!("dropping malformed message from {sender}");
            return Ok(());
        }
        if let Some(reaction) = &dm.reaction {
            if self.message_in_conversation(&reaction.target_id, conv_id)? {
                match &reaction.emoji {
                    Some(e) => self.db.borrow().conn.execute(
                        "INSERT OR REPLACE INTO reactions (message_id, account_id, emoji, ts) VALUES (?1, ?2, ?3, ?4)",
                        params![reaction.target_id, sender, e, dm.sent_at],
                    )?,
                    None => self.db.borrow().conn.execute(
                        "DELETE FROM reactions WHERE message_id = ?1 AND account_id = ?2",
                        params![reaction.target_id, sender],
                    )?,
                };
                self.emit_message(&reaction.target_id, false);
                self.vault_put_message(&reaction.target_id);
            }
            return Ok(());
        }
        if let Some(edit) = &dm.edit {
            if let Some((author, sent_at)) = self.message_author(&edit.target_id, conv_id)? {
                if author == sender && dm.sent_at - sent_at <= EDIT_WINDOW_MS {
                    let mentions = super::messages::sanitize_mentions(Some(&edit.body), &edit.mentions);
                    let mentions_json = (!mentions.is_empty()).then(|| serde_json::to_string(&mentions)).transpose()?;
                    // Keeps the previous text; past MAX_EDITS edits the message stays as is.
                    if !self.apply_edit(&edit.target_id, &edit.body, mentions_json, dm.sent_at)? {
                        return Ok(());
                    }
                    self.emit_message(&edit.target_id, false);
                    self.emit_conversation(conv_id);
                    self.vault_put_message(&edit.target_id);
                }
            }
            return Ok(());
        }
        if let Some(target) = &dm.delete {
            if let Some((author, sent_at)) = self.message_author(target, conv_id)? {
                let is_admin_delete = false;
                if (author == sender || is_admin_delete) && dm.sent_at - sent_at <= DELETE_WINDOW_MS {
                    self.mark_deleted_for_everyone(target)?;
                    self.emit_message(target, false);
                    self.emit_conversation(conv_id);
                    self.vault_put_message(target);
                }
            }
            return Ok(());
        }
        if dm.timer_update {
            if self.conversation_row(conv_id)?.group_id.is_some() {
                // Groups: the timer is a group rule changed by admins (group settings).
                return Ok(());
            }
            self.db.borrow().conn.execute(
                "UPDATE conversations SET disappearing_seconds = ?2 WHERE id = ?1",
                params![conv_id, dm.expire_timer],
            )?;
            self.insert_system_message(
                conv_id,
                "disappearing_changed",
                Some(sender.to_string()),
                vec![],
                Some(dm.expire_timer.to_string()),
                Some(dm.id.clone()),
                dm.sent_at,
            )?;
            self.expire_existing(conv_id, dm.expire_timer)?;
            self.vault_put_conversation(conv_id);
            return Ok(());
        }
        if dm.view_once {
            // Exactly one photo, video or voice note, nothing else (engine/view_once.rs).
            dm.body = None;
            dm.mentions.clear();
            dm.contacts.clear();
            dm.location = None;
            dm.attachments.truncate(1);
            if dm.attachments.len() != 1 || !super::view_once::valid_view_once_kind(&dm.attachments[0].kind) {
                log::warn!("dropping malformed view-once message from {sender}");
                return Ok(());
            }
        }
        let contacts = sanitize_contacts(&dm.contacts);
        if dm.body.as_deref().unwrap_or("").is_empty() && dm.attachments.is_empty() && dm.location.is_none() && contacts.is_empty() {
            return Ok(());
        }
        // Disappearing timer: our own messages (from another device) count from sending,
        // received ones from when they are read here (see `start_expiry_timers`).
        let expires_at = (dm.expire_timer > 0 && outgoing).then(|| dm.sent_at + dm.expire_timer * 1000);
        if expires_at.map(|t| t <= now).unwrap_or(false) {
            return Ok(()); // already expired
        }
        // Message requests and pending group invites: nothing is fetched or acknowledged.
        let accepted = self.conversation_accepted_id(conv_id);
        let auto_download: Vec<String> = dm
            .attachments
            .iter()
            .enumerate()
            .filter(|(_, a)| {
                let kind_ok = if dm.view_once {
                    !outgoing && super::view_once::valid_view_once_kind(&a.kind)
                } else {
                    a.kind == "image" || a.kind == "voice"
                };
                // Delivered after the retention limit: stored erased, nothing to fetch.
                accepted && kind_ok && a.size <= AUTO_DOWNLOAD_MAX && !self.media_past_retention(dm.sent_at, now)
            })
            .map(|(i, _)| format!("{}-{i}", dm.id))
            .collect();
        let id = dm.id.clone();
        let inserted = self.insert_message(NewMessage {
            id: dm.id.clone(),
            conversation_id: conv_id.to_string(),
            sender_account_id: sender.to_string(),
            sender_device: device,
            outgoing,
            body: dm.body.clone(),
            attachments: dm
                .attachments
                .iter()
                .enumerate()
                .map(|(i, a)| {
                    let mut st = self.stored_from_pointer(&dm.id, i, a);
                    if dm.view_once {
                        // No preview of view-once media, and our own sent copy is not kept.
                        st.thumbnail = None;
                        if outgoing {
                            st.download_state = "done".into();
                        }
                    }
                    st
                })
                .collect(),
            location: dm.location.clone(),
            contacts,
            mentions: super::messages::sanitize_mentions(dm.body.as_deref(), &dm.mentions),
            reply_to_id: dm.quote.clone(),
            forwarded: dm.forwarded,
            status: if outgoing { "sent".into() } else { "delivered".into() },
            sent_at: dm.sent_at,
            received_at: now,
            expires_in: dm.expire_timer,
            expires_at,
            system: None,
        })?;
        if !inserted {
            return Ok(());
        }
        if dm.view_once {
            self.mark_view_once(&id)?;
        }
        if !outgoing {
            self.clear_undecryptable(sender, dm.sent_at)?;
        }
        if !outgoing && accepted {
            self.enqueue(&Job::Account {
                account_id: sender.to_string(),
                content: Content::Receipt { kind: ReceiptKind::Delivered, ids: vec![id.clone()] },
            })?;
        }
        self.emit_message(&id, !outgoing);
        self.emit_conversation(conv_id);
        self.vault_put_message(&id);
        for att_id in auto_download {
            let me = self.clone();
            let mid = id.clone();
            tokio::task::spawn_local(async move {
                if let Err(e) = me.download_attachment(&mid, &att_id).await {
                    log::warn!("auto-download {att_id}: {e}");
                }
            });
        }
        Ok(())
    }

    fn message_in_conversation(&self, id: &str, conv_id: &str) -> Result<bool> {
        Ok(self
            .db
            .borrow()
            .conn
            .query_row(
                "SELECT 1 FROM messages WHERE id = ?1 AND conversation_id = ?2",
                params![id, conv_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    fn message_author(&self, id: &str, conv_id: &str) -> Result<Option<(String, i64)>> {
        Ok(self
            .db
            .borrow()
            .conn
            .query_row(
                "SELECT sender_account_id, sent_at FROM messages WHERE id = ?1 AND conversation_id = ?2",
                params![id, conv_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?)
    }

    /// Records delivery/read receipts from `from` for our outgoing messages.
    fn apply_receipts(&self, from: &str, kind: ReceiptKind, ids: &[String], ts: i64) -> Result<()> {
        let me = self.me()?;
        let show_read = self.privacy()?.read_receipts;
        for id in ids {
            let conv: Option<String> = self
                .db
                .borrow()
                .conn
                .query_row(
                    "SELECT conversation_id FROM messages WHERE id = ?1 AND sender_account_id = ?2",
                    params![id, me],
                    |r| r.get(0),
                )
                .optional()?;
            let Some(conv) = conv else { continue };
            // In groups, receipts only from attested members.
            if let Ok(row) = self.conversation_row(&conv) {
                if row.group_id.as_deref().is_some_and(|g| !self.is_attested_member(g, from)) {
                    continue;
                }
            }
            {
                let db = self.db.borrow();
                db.conn.execute(
                    "INSERT OR IGNORE INTO receipts (message_id, account_id) VALUES (?1, ?2)",
                    params![id, from],
                )?;
                match kind {
                    ReceiptKind::Delivered => db.conn.execute(
                        "UPDATE receipts SET delivered_at = COALESCE(delivered_at, ?3) WHERE message_id = ?1 AND account_id = ?2",
                        params![id, from, ts],
                    )?,
                    ReceiptKind::Read | ReceiptKind::Viewed => db.conn.execute(
                        "UPDATE receipts SET delivered_at = COALESCE(delivered_at, ?3), read_at = COALESCE(read_at, ?3) WHERE message_id = ?1 AND account_id = ?2",
                        params![id, from, ts],
                    )?,
                };
            }
            if kind == ReceiptKind::Viewed {
                self.on_view_once_viewed(id)?;
            }
            let status = self.aggregate_status(id, &conv, show_read)?;
            self.set_message_status(id, &status)?;
        }
        Ok(())
    }
}

/// Keeps well-formed shared contacts only (bounded count and field sizes).
fn sanitize_contacts(contacts: &[SharedContact]) -> Vec<SharedContact> {
    contacts
        .iter()
        .filter(|c| {
            !c.account_id.is_empty()
                && c.account_id.len() <= 64
                && c.account_id.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
                && c.number.len() == 16
                && c.number.chars().all(|ch| ch.is_ascii_digit())
        })
        .take(super::messages::MAX_SHARED_CONTACTS)
        .map(|c| SharedContact {
            account_id: c.account_id.clone(),
            number: c.number.clone(),
            name: crate::sanitize::clean_display_text(&c.name, 100),
        })
        .collect()
}

/// Receive-side limits (the sender's app enforces the same, a modified one may not): text
/// and edits up to `MAX_TEXT` characters, at most `MAX_ATTACHMENTS` attachments, a reaction
/// is one emoji, a location has valid coordinates and short labels. Returns false when the
/// message must be dropped.
fn limit_incoming(dm: &mut DataMessage) -> bool {
    use super::messages::{MAX_ATTACHMENTS, MAX_TEXT};
    use crate::sanitize::{clean_display_text, is_single_emoji, truncate_chars};
    if let Some(body) = dm.body.as_mut() {
        truncate_chars(body, MAX_TEXT);
    }
    if let Some(edit) = dm.edit.as_mut() {
        truncate_chars(&mut edit.body, MAX_TEXT);
        if edit.body.trim().is_empty() {
            return false;
        }
    }
    if dm.reaction.as_ref().and_then(|r| r.emoji.as_deref()).is_some_and(|e| !is_single_emoji(e)) {
        return false;
    }
    dm.attachments.truncate(MAX_ATTACHMENTS);
    dm.expire_timer = dm.expire_timer.clamp(0, 10 * 365 * 24 * 3600);
    if let Some(loc) = dm.location.as_mut() {
        let valid = loc.lat.is_finite() && loc.lng.is_finite() && loc.lat.abs() <= 90.0 && loc.lng.abs() <= 180.0;
        if !valid {
            dm.location = None;
        } else {
            loc.name = loc.name.as_deref().map(|n| clean_display_text(n, 200)).filter(|n| !n.is_empty());
            loc.address = loc.address.as_deref().map(|a| clean_display_text(a, 500)).filter(|a| !a.is_empty());
        }
    }
    true
}
