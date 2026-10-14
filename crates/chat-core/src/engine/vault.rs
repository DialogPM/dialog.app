//! Encrypted server vault: syncs contacts, group keys, conversation settings and (optionally)
//! message history between the devices of an account. Records are encrypted with the vault
//! key derived from the account secret; record ids are HMACs of logical ids.

use std::rc::Rc;
use std::time::Duration;

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::messages::{MessageRow, NewMessage};
use super::Inner;
use crate::api::{VaultPageDto, VaultRecordDto};
use crate::content::{Content, SyncMessage};
use crate::crypto::{self, b64, unb64};
use crate::error::Result;
use crate::model::*;
use crate::util::now_ms;

const VAULT_AAD: &[u8] = b"chatapp-vault-v1";
const BATCH: usize = 200;

/// Where a conversation points to (stable across devices, unlike local conversation ids).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
enum Target {
    Peer { account_id: String },
    Group { group_id: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "r", rename_all = "snake_case")]
enum Record {
    Deleted {
        lid: String,
    },
    Settings {
        privacy: PrivacySettings,
        /// Our rotated profile key (engine/profile_key.rs), so new devices use it too.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        profile_key: Option<String>,
        /// Do not disturb (engine/dnd.rs); absent in records from older versions.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        dnd: Option<super::dnd::DndSetting>,
        /// Call privacy settings (engine/call_settings.rs); absent in records from older versions.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        calls: Option<super::call_settings::CallSettingsRecord>,
        /// Media retention period (engine/retention.rs); absent in records from older versions.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        media_retention: Option<super::retention::MediaRetentionRecord>,
        /// Default disappearing timer (engine/default_timer.rs); absent in records from older versions.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        default_timer: Option<super::default_timer::DefaultTimerRecord>,
    },
    Contact {
        account_id: String,
        number: String,
        local_name: Option<String>,
        phone: Option<String>,
        blocked: bool,
        verified: bool,
        is_contact: bool,
        profile_key: Option<String>,
        updated_at: i64,
        /// Pinned Signal identity key (base64), so a newly linked device starts with the
        /// identities this account already trusts instead of trusting the server.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        identity_key: Option<String>,
    },
    Group {
        group_id: String,
        group_key: String,
        /// End-to-end attested member list (engine/attest.rs), so a newly linked device does
        /// not have to take the server's list.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        attested: Option<super::attest::Attested>,
    },
    Conversation {
        target: Target,
        muted_until: Option<i64>,
        archived: bool,
        pinned: bool,
        disappearing_seconds: i64,
        /// Message request accepted (records from older versions: everything was accepted).
        #[serde(default = "accepted_default")]
        accepted: bool,
    },
    Message(Box<VaultMessage>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct VaultMessage {
    id: String,
    target: Target,
    sender_account_id: String,
    outgoing: bool,
    body: Option<String>,
    attachments: Vec<StoredAttachment>,
    location: Option<Location>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    contacts: Vec<SharedContact>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    mentions: Vec<Mention>,
    reply_to_id: Option<String>,
    forwarded: bool,
    status: String,
    sent_at: i64,
    received_at: i64,
    edited_at: Option<i64>,
    deleted: bool,
    system: Option<StoredSystemEvent>,
    reactions: Vec<(String, String)>,
    read: bool,
}

impl Inner {
    fn mark_dirty(&self, lid: &str) {
        let _ = self
            .db
            .borrow()
            .conn
            .execute("INSERT OR IGNORE INTO vault_dirty (logical_id) VALUES (?1)", [lid]);
        self.vault_notify.notify_one();
    }

    pub fn vault_put_message(&self, id: &str) {
        if !self.backup_enabled() || self.temporary_mode() {
            return;
        }
        // Disappearing messages are never backed up (and must not produce tombstones either,
        // which would delete them on every device, including this one).
        if matches!(self.message_row(id), Ok(Some(m)) if m.is_disappearing()) {
            return;
        }
        self.mark_dirty(&format!("msg:{id}"));
    }

    pub fn vault_put_conversation(&self, conv_id: &str) {
        if self.temporary_mode() {
            return;
        }
        if let Ok(row) = self.conversation_row(conv_id) {
            if let Some(p) = row.peer_account_id {
                self.mark_dirty(&format!("conv:peer:{p}"));
            } else if let Some(g) = row.group_id {
                self.mark_dirty(&format!("conv:group:{g}"));
            }
        }
    }

    pub fn vault_put_contact(&self, account_id: &str) {
        self.mark_dirty(&format!("contact:{account_id}"));
    }

    pub fn vault_put_group(&self, group_id: &str) {
        self.mark_dirty(&format!("group:{group_id}"));
    }

    pub async fn vault_put_settings(&self) {
        self.mark_dirty("settings");
    }

    /// Marks a logical record as deleted (the flush will write a tombstone).
    pub fn vault_delete(&self, lid: &str) {
        self.mark_dirty(lid);
    }

    pub fn vault_backfill_messages(&self) -> Result<()> {
        self.db.borrow().conn.execute(
            "INSERT OR IGNORE INTO vault_dirty (logical_id) SELECT 'msg:' || id FROM messages WHERE expires_at IS NULL AND expires_in = 0 AND view_once = 0",
            [],
        )?;
        Ok(())
    }

    fn target_of(&self, conv_id: &str) -> Option<Target> {
        let row = self.conversation_row(conv_id).ok()?;
        match (row.peer_account_id, row.group_id) {
            (Some(p), _) => Some(Target::Peer { account_id: p }),
            (None, Some(g)) => Some(Target::Group { group_id: g }),
            _ => None,
        }
    }

    /// True for a disappearing message that still exists locally.
    fn is_ephemeral_message(&self, lid: &str) -> Result<bool> {
        Ok(match lid.strip_prefix("msg:") {
            Some(id) => matches!(self.message_row(id)?, Some(m) if m.is_disappearing()),
            None => false,
        })
    }

    /// Builds the current record for a logical id (None = nothing to store, tombstone).
    fn build_record(&self, lid: &str) -> Result<Option<Record>> {
        let db = self.db.borrow();
        if lid == "settings" {
            drop(db);
            return Ok(Some(Record::Settings {
                privacy: self.privacy()?,
                profile_key: self.rotated_profile_key_b64(),
                dnd: Some(self.dnd_setting()).filter(|d| d.updated_at > 0),
                calls: Some(self.call_settings_record()).filter(|c| c.updated_at > 0),
                media_retention: Some(self.media_retention_record()).filter(|r| r.updated_at > 0),
                default_timer: Some(self.default_timer_record()).filter(|r| r.updated_at > 0),
            }));
        }
        if let Some(id) = lid.strip_prefix("contact:") {
            let r = db
                .conn
                .query_row(
                    "SELECT number, local_name, phone, blocked, verified, is_contact, profile_key, updated_at FROM contacts WHERE account_id = ?1",
                    [id],
                    |r| {
                        Ok(Record::Contact {
                            account_id: id.to_string(),
                            number: r.get(0)?,
                            local_name: r.get(1)?,
                            phone: r.get(2)?,
                            blocked: r.get(3)?,
                            verified: r.get(4)?,
                            is_contact: r.get(5)?,
                            profile_key: r.get::<_, Option<Vec<u8>>>(6)?.map(b64),
                            updated_at: r.get(7)?,
                            identity_key: None,
                        })
                    },
                )
                .optional()?;
            let identity: Option<Vec<u8>> = db
                .conn
                .query_row("SELECT identity_key FROM identities WHERE account_id = ?1", [id], |r| r.get(0))
                .optional()?;
            return Ok(r.map(|rec| match rec {
                Record::Contact { identity_key: _, account_id, number, local_name, phone, blocked, verified, is_contact, profile_key, updated_at } => {
                    Record::Contact { identity_key: identity.map(b64), account_id, number, local_name, phone, blocked, verified, is_contact, profile_key, updated_at }
                }
                other => other,
            }));
        }
        if let Some(gid) = lid.strip_prefix("group:") {
            let key: Option<Option<Vec<u8>>> = db
                .conn
                .query_row("SELECT group_key FROM groups WHERE group_id = ?1", [gid], |r| r.get(0))
                .optional()?;
            let attested = db.kv_get_json::<super::attest::Attested>(&format!("group_attested:{gid}")).ok().flatten();
            return Ok(key.flatten().map(|k| Record::Group { group_id: gid.to_string(), group_key: b64(k), attested }));
        }
        if let Some(rest) = lid.strip_prefix("conv:") {
            let (col, target) = if let Some(p) = rest.strip_prefix("peer:") {
                ("peer_account_id", Target::Peer { account_id: p.to_string() })
            } else if let Some(g) = rest.strip_prefix("group:") {
                ("group_id", Target::Group { group_id: g.to_string() })
            } else {
                return Ok(None);
            };
            let key = rest.split_once(':').map(|(_, k)| k).unwrap_or_default();
            let r = db
                .conn
                .query_row(
                    &format!("SELECT muted_until, archived, pinned, disappearing_seconds, accepted FROM conversations WHERE {col} = ?1"),
                    [key],
                    |r| {
                        Ok(Record::Conversation {
                            target: target.clone(),
                            muted_until: r.get(0)?,
                            archived: r.get(1)?,
                            pinned: r.get(2)?,
                            disappearing_seconds: r.get(3)?,
                            accepted: r.get(4)?,
                        })
                    },
                )
                .optional()?;
            return Ok(r);
        }
        if let Some(id) = lid.strip_prefix("msg:") {
            drop(db);
            let Some(m) = self.message_row(id)? else { return Ok(None) };
            if m.is_disappearing() {
                return Ok(None); // disappearing messages are never backed up
            }
            let Some(target) = self.target_of(&m.conversation_id) else { return Ok(None) };
            let reactions: Vec<(String, String)> = self
                .db
                .borrow()
                .conn
                .prepare("SELECT account_id, emoji FROM reactions WHERE message_id = ?1")?
                .query_map([id], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?;
            let read: bool = self
                .db
                .borrow()
                .conn
                .query_row("SELECT read_local FROM messages WHERE id = ?1", [id], |r| r.get(0))?;
            return Ok(Some(Record::Message(Box::new(vault_message(m, target, reactions, read)))));
        }
        Ok(None)
    }

    async fn vault_flush(&self) -> Result<()> {
        let s = self.session()?;
        loop {
            let lids: Vec<String> = self
                .db
                .borrow()
                .conn
                .prepare("SELECT logical_id FROM vault_dirty LIMIT ?1")?
                .query_map([BATCH as i64], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            if lids.is_empty() {
                return Ok(());
            }
            let mut records = Vec::with_capacity(lids.len());
            let mut notify_devices = false;
            for lid in &lids {
                if self.is_ephemeral_message(lid)? {
                    continue; // exists but must stay local only
                }
                let record = self.build_record(lid)?.unwrap_or(Record::Deleted { lid: lid.clone() });
                if !lid.starts_with("msg:") {
                    notify_devices = true;
                }
                let sealed = crypto::aead_seal(&s.keys.vault_key, &serde_json::to_vec(&record)?, VAULT_AAD)?;
                records.push(VaultRecordDto {
                    id: crypto::vault_record_id(&s.keys.vault_key, lid),
                    data: Some(b64(sealed)),
                    seq: 0,
                });
            }
            if !records.is_empty() {
                let _: serde_json::Value = s.api.put("/v1/vault/records", &json!({ "records": records })).await?;
            }
            {
                let db = self.db.borrow();
                for lid in &lids {
                    db.conn.execute("DELETE FROM vault_dirty WHERE logical_id = ?1", [lid])?;
                }
            }
            if notify_devices {
                self.enqueue(&super::send::Job::OwnDevices { content: Content::Sync(SyncMessage::VaultUpdated) })?;
            }
        }
    }

    async fn vault_pull(&self) -> Result<()> {
        let s = self.session()?;
        let restoring: bool = self.db.borrow().kv_get_json("restore_pending")?.unwrap_or(false);
        let mut applied = 0usize;
        loop {
            let since: i64 = self.db.borrow().kv_get_json("vault_seq")?.unwrap_or(0);
            let page: VaultPageDto = s.api.get(&format!("/v1/vault/records?since={since}&limit=500")).await?;
            let mut max_seq = since;
            for r in &page.records {
                max_seq = max_seq.max(r.seq);
                let Some(data) = &r.data else { continue };
                let plain = match unb64(data).and_then(|d| crypto::aead_open(&s.keys.vault_key, &d, VAULT_AAD)) {
                    Ok(p) => p,
                    Err(e) => {
                        log::warn!("vault record {}: {e}", r.id);
                        continue;
                    }
                };
                match serde_json::from_slice::<Record>(&plain) {
                    Ok(rec) => {
                        if let Err(e) = self.apply_record(rec).await {
                            log::warn!("vault apply: {e}");
                        }
                        applied += 1;
                    }
                    Err(e) => log::warn!("vault decode: {e}"),
                }
            }
            self.db.borrow().kv_set_json("vault_seq", &max_seq)?;
            if restoring {
                self.emit("history_sync", &json!({ "done": false, "records": applied }));
            }
            if !page.more {
                break;
            }
        }
        if restoring {
            self.db.borrow().kv_set_json("restore_pending", &false)?;
            self.emit("history_sync", &json!({ "done": true, "records": applied }));
            // Pull groups and profiles now that keys are known.
            self.sync_all_groups().await;
            let me = self.me()?;
            let _ = self.fetch_profile(&me).await;
            self.refresh_stale_profiles().await;
        }
        Ok(())
    }

    async fn conv_for_target(&self, target: &Target) -> Result<String> {
        match target {
            Target::Peer { account_id } => self.ensure_direct_conversation(account_id, false).await,
            Target::Group { group_id } => self.create_group_conversation(group_id),
        }
    }

    async fn apply_record(&self, rec: Record) -> Result<()> {
        match rec {
            Record::Deleted { lid } => {
                if let Some(id) = lid.strip_prefix("msg:") {
                    // Disappearing messages expire on their own; ignore (old, buggy) tombstones.
                    if matches!(self.message_row(id)?, Some(m) if !m.is_disappearing()) {
                        // Deleted on another device.
                        let conv = self.message_row(id)?.map(|m| m.conversation_id);
                        self.drop_marks(id);
                        self.db.borrow().conn.execute("DELETE FROM messages WHERE id = ?1", [id])?;
                        if let Some(c) = conv {
                            self.emit("message_deleted", &json!({ "conversation_id": c, "message_id": id }));
                            self.emit_conversation(&c);
                        }
                    }
                }
            }
            Record::Settings { privacy, profile_key, dnd, calls, media_retention, default_timer } => {
                if let Some(t) = default_timer {
                    self.apply_synced_default_timer(t)?;
                }
                if let Some(r) = media_retention {
                    self.apply_synced_media_retention(r)?;
                }
                self.db.borrow().kv_set_json("privacy", &privacy)?;
                if let Some(d) = dnd {
                    self.apply_synced_dnd(d)?;
                }
                if let Some(c) = calls {
                    self.apply_synced_call_settings(c)?;
                }
                if let Some(k) = profile_key.as_deref().and_then(|k| unb64(k).ok()) {
                    self.set_rotated_profile_key(&k)?;
                }
            }
            Record::Contact { account_id, number, local_name, phone, blocked, verified, is_contact, profile_key, updated_at, identity_key } => {
                if let Some(key) = identity_key.as_deref().and_then(|k| unb64(k).ok()) {
                    self.pin_identity_from_vault(&account_id, &key)?;
                }
                let local_updated: Option<i64> = self
                    .db
                    .borrow()
                    .conn
                    .query_row("SELECT updated_at FROM contacts WHERE account_id = ?1", [&account_id], |r| r.get(0))
                    .optional()?;
                if local_updated.map(|u| u > updated_at).unwrap_or(false) {
                    return Ok(());
                }
                let pk = profile_key.and_then(|k| unb64(&k).ok());
                self.db.borrow().conn.execute(
                    "INSERT INTO contacts (account_id, number, local_name, phone, blocked, verified, is_contact, profile_key, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                     ON CONFLICT(account_id) DO UPDATE SET number = CASE WHEN excluded.number = '' THEN contacts.number ELSE excluded.number END,
                        local_name = excluded.local_name, phone = excluded.phone, blocked = excluded.blocked,
                        verified = excluded.verified, is_contact = excluded.is_contact,
                        profile_key = COALESCE(excluded.profile_key, contacts.profile_key), updated_at = excluded.updated_at",
                    params![account_id, number, local_name, phone, blocked, verified, is_contact, pk, updated_at],
                )?;
                if let Ok(c) = self.contact_model(&account_id) {
                    self.emit("contact_updated", &c);
                }
            }
            Record::Group { group_id, group_key, attested } => {
                // Our own devices' list wins over a TOFU one, never over a newer snapshot.
                if let Some(a) = attested {
                    let take = match self.attested(&group_id) {
                        None => true,
                        Some(cur) => cur.tofu || a.revision >= cur.revision,
                    };
                    if take {
                        self.set_attested(&group_id, &a)?;
                    }
                }
                if self.store_group_key(&group_id, &unb64(&group_key)?)? {
                    let _ = self.sync_group(&group_id, None).await;
                }
            }
            Record::Conversation { target, muted_until, archived, pinned, disappearing_seconds, accepted } => {
                let conv = self.conv_for_target(&target).await?;
                // Acceptance only ever goes one way (a stale record must not turn a chat back
                // into a request).
                self.db.borrow().conn.execute(
                    "UPDATE conversations SET muted_until = ?2, archived = ?3, pinned = ?4, disappearing_seconds = ?5,
                        accepted = MAX(accepted, ?6) WHERE id = ?1",
                    params![conv, muted_until, archived, pinned, disappearing_seconds, accepted],
                )?;
                self.emit_conversation(&conv);
            }
            Record::Message(m) => {
                if !crate::util::is_id(&m.id) {
                    return Ok(());
                }
                let conv = self.conv_for_target(&m.target).await?;
                let exists = self.message_row(&m.id)?;
                if let Some(existing) = exists {
                    // A record only moves a message forward (newer edit, deletion): a stale copy
                    // uploaded before an edit must not bring the old text back.
                    let newer_edit = match (m.edited_at, existing.edited_at) {
                        (Some(a), Some(b)) => a > b,
                        (Some(_), None) => true,
                        _ => false,
                    };
                    if m.deleted && !existing.deleted {
                        self.db.borrow().conn.execute(
                            "UPDATE messages SET deleted = 1, body = NULL, edit_history_json = NULL WHERE id = ?1",
                            [&m.id],
                        )?;
                    } else if newer_edit && !existing.deleted {
                        let mentions_json = (!m.mentions.is_empty()).then(|| serde_json::to_string(&m.mentions)).transpose()?;
                        self.apply_edit(&m.id, m.body.as_deref().unwrap_or(""), mentions_json, m.edited_at.unwrap_or(m.sent_at))?;
                    }
                    let db = self.db.borrow();
                    db.conn.execute("UPDATE messages SET read_local = MAX(read_local, ?2) WHERE id = ?1", params![m.id, m.read])?;
                    let rank = |s: &str| ["failed", "pending", "sent", "delivered", "read"].iter().position(|x| *x == s).unwrap_or(0);
                    // Our other devices store their sync copy as "sent" even while this device
                    // still waits for the recipient to have a device (engine/send.rs).
                    let waiting = existing.status_reason.as_deref() == Some("no_devices") && existing.status == "pending";
                    if rank(&m.status) > rank(&existing.status) && !waiting {
                        db.conn.execute("UPDATE messages SET status = ?2 WHERE id = ?1", params![m.id, m.status])?;
                    }
                } else {
                    // Media past the retention limit is stored erased by `insert_message`.
                    let attachments = m.attachments.iter().filter_map(restored_attachment).collect();
                    self.insert_message(NewMessage {
                        id: m.id.clone(),
                        conversation_id: conv.clone(),
                        sender_account_id: m.sender_account_id.clone(),
                        sender_device: 0,
                        outgoing: m.outgoing,
                        body: if m.deleted { None } else { m.body.clone() },
                        attachments,
                        location: m.location.clone(),
                        contacts: m.contacts.clone(),
                        mentions: m.mentions.clone(),
                        reply_to_id: m.reply_to_id.clone(),
                        forwarded: m.forwarded,
                        status: m.status.clone(),
                        sent_at: m.sent_at,
                        received_at: m.received_at,
                        expires_in: 0,
                        expires_at: None,
                        system: m.system.clone(),
                    })?;
                    let db = self.db.borrow();
                    db.conn.execute(
                        "UPDATE messages SET read_local = ?2, edited_at = ?3, deleted = ?4 WHERE id = ?1",
                        params![m.id, m.read || m.outgoing, m.edited_at, m.deleted],
                    )?;
                }
                {
                    let db = self.db.borrow();
                    db.conn.execute("DELETE FROM reactions WHERE message_id = ?1", [&m.id])?;
                    for (acct, emoji) in &m.reactions {
                        db.conn.execute(
                            "INSERT OR REPLACE INTO reactions (message_id, account_id, emoji, ts) VALUES (?1, ?2, ?3, ?4)",
                            params![m.id, acct, emoji, m.sent_at],
                        )?;
                    }
                }
                self.recount_unread(&conv)?;
                self.emit_message(&m.id, false);
                self.emit_conversation(&conv);
            }
        }
        Ok(())
    }

    pub(super) async fn vault_loop(self: Rc<Self>) {
        let mut pulled_once = false;
        loop {
            if self.ws_out.borrow().is_some() {
                if let Err(e) = self.vault_flush().await {
                    log::info!("vault flush: {e}");
                }
                if let Err(e) = self.vault_pull().await {
                    log::info!("vault pull: {e}");
                } else {
                    pulled_once = true;
                }
            }
            let notify = self.vault_notify.clone();
            let wait = if pulled_once { 300 } else { 10 };
            tokio::select! {
                _ = notify.notified() => {
                    // Coalesce bursts of writes.
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
                _ = tokio::time::sleep(Duration::from_secs(wait)) => {}
            }
            let _ = now_ms();
        }
    }
}

/// An attachment as backed up / restored: uploaded ones as a pointer to download again, media
/// erased by the retention (engine/retention.rs) as its stub (no key, nothing to download).
fn restored_attachment(a: &StoredAttachment) -> Option<StoredAttachment> {
    if super::retention::is_expired(a) {
        Some(super::retention::expired_attachment(a, a.expired_at.unwrap_or_else(now_ms)))
    } else if a.cdn_id.is_some() {
        Some(StoredAttachment { local_path: None, download_state: "pending".into(), ..a.clone() })
    } else {
        None
    }
}

fn vault_message(m: MessageRow, target: Target, reactions: Vec<(String, String)>, read: bool) -> VaultMessage {
    VaultMessage {
        id: m.id,
        target,
        sender_account_id: m.sender_account_id,
        outgoing: m.outgoing,
        body: m.body,
        attachments: m.attachments.iter().filter_map(restored_attachment).collect(),
        location: m.location,
        contacts: m.contacts,
        mentions: m.mentions,
        reply_to_id: m.reply_to_id,
        forwarded: m.forwarded,
        status: m.status,
        sent_at: m.sent_at,
        received_at: m.received_at,
        edited_at: m.edited_at,
        deleted: m.deleted,
        system: m.system,
        reactions,
        read,
    }
}

impl Inner {
    /// Pins an identity learned by another of our devices; never replaces a pinned one.
    fn pin_identity_from_vault(&self, account_id: &str, key: &[u8]) -> Result<()> {
        let db = self.db.borrow();
        let existing: Option<Vec<u8>> = db
            .conn
            .query_row("SELECT identity_key FROM identities WHERE account_id = ?1", [account_id], |r| r.get(0))
            .optional()?;
        match existing {
            None => {
                db.conn.execute(
                    "INSERT INTO identities (account_id, identity_key, first_seen) VALUES (?1, ?2, ?3)",
                    params![account_id, key, crate::util::now_ms()],
                )?;
            }
            Some(old) if old != key => {
                log::error!("vault identity for {account_id} differs from the pinned one; keeping the pinned key");
            }
            Some(_) => {}
        }
        Ok(())
    }
}

fn accepted_default() -> bool {
    true
}
