//! Local message storage and user-facing message operations.

use std::collections::{BTreeMap, HashSet};
use std::rc::Rc;
use std::time::Duration;

use rusqlite::{params, OptionalExtension, Row};

use super::send::Job;
use super::Inner;
use crate::content::{Content, DataMessage, EditAction, ReactionAction, ReceiptKind, SyncMessage};
use crate::error::{CoreError, ErrorCode, Result};
use crate::model::*;
use crate::util::{new_id, now_ms};

const EDIT_WINDOW_MS: i64 = 24 * 3600 * 1000;
const DELETE_WINDOW_MS: i64 = 2 * 24 * 3600 * 1000;
/// Longest message text, in characters (also enforced on received messages and edits).
pub const MAX_TEXT: usize = 65536;
/// Most attachments in one message (received extras are dropped).
pub const MAX_ATTACHMENTS: usize = 32;
pub const MAX_SHARED_CONTACTS: usize = 50;

pub struct NewMessage {
    pub id: String,
    pub conversation_id: String,
    pub sender_account_id: String,
    pub sender_device: u32,
    pub outgoing: bool,
    pub body: Option<String>,
    pub attachments: Vec<StoredAttachment>,
    pub location: Option<Location>,
    pub contacts: Vec<SharedContact>,
    pub mentions: Vec<Mention>,
    pub reply_to_id: Option<String>,
    pub forwarded: bool,
    pub status: String,
    pub sent_at: i64,
    pub received_at: i64,
    pub expires_in: i64,
    pub expires_at: Option<i64>,
    pub system: Option<StoredSystemEvent>,
}

/// A raw row of the messages table.
#[derive(Debug, Clone)]
pub struct MessageRow {
    pub id: String,
    pub conversation_id: String,
    pub sender_account_id: String,
    pub outgoing: bool,
    pub kind: String,
    pub body: Option<String>,
    pub attachments: Vec<StoredAttachment>,
    pub location: Option<Location>,
    pub contacts: Vec<SharedContact>,
    pub mentions: Vec<Mention>,
    pub reply_to_id: Option<String>,
    pub forwarded: bool,
    pub status: String,
    pub sent_at: i64,
    pub received_at: i64,
    pub edited_at: Option<i64>,
    pub deleted: bool,
    pub expires_in: i64,
    pub expires_at: Option<i64>,
    pub system: Option<StoredSystemEvent>,
    /// View-once state (engine/view_once.rs): 0 = normal message.
    pub view_once: i64,
    /// Why an outgoing message is pending or failed ("no_devices", "account_gone").
    pub status_reason: Option<String>,
}

impl MessageRow {
    /// A disappearing message (its timer may not have started yet: received, still unread),
    /// or view-once media. Such messages are never backed up to the vault.
    pub fn is_disappearing(&self) -> bool {
        self.expires_at.is_some() || self.expires_in > 0 || self.view_once != 0
    }
}

pub const MESSAGE_COLUMNS: &str = "id, conversation_id, sender_account_id, outgoing, kind, body, attachments_json, \
    location_json, reply_to_id, forwarded, status, sent_at, received_at, edited_at, deleted, expires_in, expires_at, system_json, contacts_json, mentions_json, view_once, status_reason";

pub fn message_from_row(r: &Row) -> rusqlite::Result<MessageRow> {
    let atts: String = r.get(6)?;
    let loc: Option<String> = r.get(7)?;
    let sys: Option<String> = r.get(17)?;
    let contacts: Option<String> = r.get(18)?;
    let mentions: Option<String> = r.get(19)?;
    Ok(MessageRow {
        id: r.get(0)?,
        conversation_id: r.get(1)?,
        sender_account_id: r.get(2)?,
        outgoing: r.get(3)?,
        kind: r.get(4)?,
        body: r.get(5)?,
        attachments: serde_json::from_str(&atts).unwrap_or_default(),
        location: loc.and_then(|l| serde_json::from_str(&l).ok()),
        contacts: contacts.and_then(|c| serde_json::from_str(&c).ok()).unwrap_or_default(),
        mentions: mentions.and_then(|c| serde_json::from_str(&c).ok()).unwrap_or_default(),
        reply_to_id: r.get(8)?,
        forwarded: r.get(9)?,
        status: r.get(10)?,
        sent_at: r.get(11)?,
        received_at: r.get(12)?,
        edited_at: r.get(13)?,
        deleted: r.get(14)?,
        expires_in: r.get(15)?,
        expires_at: r.get(16)?,
        system: sys.and_then(|s| serde_json::from_str(&s).ok()),
        view_once: r.get(20)?,
        status_reason: r.get(21)?,
    })
}

fn status_rank(s: &str) -> i32 {
    match s {
        "failed" => 0,
        "pending" => 1,
        "sent" => 2,
        "delivered" => 3,
        "read" => 4,
        _ => 0,
    }
}

fn kind_of(attachments: &[StoredAttachment], location: &Option<Location>, contacts: &[SharedContact]) -> String {
    if location.is_some() {
        "location".into()
    } else if !contacts.is_empty() && attachments.is_empty() {
        "contact".into()
    } else if let Some(a) = attachments.first() {
        match a.kind.as_str() {
            "image" | "video" | "voice" => a.kind.clone(),
            _ => "file".into(),
        }
    } else {
        "text".into()
    }
}

impl Inner {
    /// Inserts a message; returns false if a message with the same id already exists.
    pub fn insert_message(&self, mut m: NewMessage) -> Result<bool> {
        // Media already past the retention limit (delivered late, restored from the vault) is
        // stored erased: nothing to download (engine/retention.rs).
        if m.system.is_none()
            && !m.attachments.is_empty()
            && self.media_past_retention(m.sent_at, m.received_at)
        {
            let now = now_ms();
            m.attachments = m.attachments.iter().map(|a| super::retention::expired_attachment(a, now)).collect();
        }
        // A message from before an admin cleared the group's history (arriving late, e.g.
        // resent after a decryption failure) stays cleared.
        if m.system.is_none() {
            let cleared: Option<i64> = self.db.borrow().kv_get_json(&format!("history_cleared:{}", m.conversation_id))?;
            if cleared.is_some_and(|before| m.sent_at <= before) {
                return Ok(false);
            }
        }
        if m.expires_at.is_some() {
            // The expiry loop may be sleeping past this message's deadline.
            self.expiry_notify.notify_one();
        }
        let kind = if m.system.is_some() { "system".to_string() } else { kind_of(&m.attachments, &m.location, &m.contacts) };
        let inserted = {
            let db = self.db.borrow();
            let n = db.conn.execute(
                "INSERT OR IGNORE INTO messages (id, conversation_id, sender_account_id, sender_device, outgoing, kind, body,
                    attachments_json, location_json, reply_to_id, forwarded, status, sent_at, received_at, expires_in,
                    expires_at, system_json, read_local, contacts_json, mentions_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)",
                params![
                    m.id,
                    m.conversation_id,
                    m.sender_account_id,
                    m.sender_device,
                    m.outgoing,
                    kind,
                    m.body,
                    serde_json::to_string(&m.attachments)?,
                    m.location.as_ref().map(serde_json::to_string).transpose()?,
                    m.reply_to_id,
                    m.forwarded,
                    m.status,
                    m.sent_at,
                    m.received_at,
                    m.expires_in,
                    m.expires_at,
                    m.system.as_ref().map(serde_json::to_string).transpose()?,
                    m.outgoing || m.system.is_some(),
                    (!m.contacts.is_empty()).then(|| serde_json::to_string(&m.contacts)).transpose()?,
                    (!m.mentions.is_empty()).then(|| serde_json::to_string(&m.mentions)).transpose()?,
                ],
            )?;
            n > 0
        };
        if inserted {
            let db = self.db.borrow();
            db.conn.execute(
                "UPDATE conversations SET updated_at = MAX(updated_at, ?2), hidden = 0 WHERE id = ?1",
                params![m.conversation_id, m.sent_at.min(m.received_at + 60_000)],
            )?;
            if !m.outgoing && m.system.is_none() {
                db.conn.execute(
                    "UPDATE conversations SET unread_count = unread_count + 1, archived = CASE WHEN muted_until IS NULL THEN 0 ELSE archived END WHERE id = ?1",
                    [&m.conversation_id],
                )?;
            }
        }
        Ok(inserted)
    }

    pub fn message_row(&self, id: &str) -> Result<Option<MessageRow>> {
        Ok(self
            .db
            .borrow()
            .conn
            .query_row(&format!("SELECT {MESSAGE_COLUMNS} FROM messages WHERE id = ?1"), [id], message_from_row)
            .optional()?)
    }

    fn require_message(&self, id: &str) -> Result<MessageRow> {
        self.message_row(id)?.ok_or_else(|| CoreError::not_found("message not found"))
    }

    pub fn update_attachments(&self, id: &str, attachments: &[StoredAttachment]) -> Result<()> {
        self.db.borrow().conn.execute(
            "UPDATE messages SET attachments_json = ?2 WHERE id = ?1",
            params![id, serde_json::to_string(attachments)?],
        )?;
        Ok(())
    }

    /// Converts a stored row to the UI model.
    pub fn message_model(&self, row: &MessageRow) -> Result<Message> {
        let me = self.me().unwrap_or_default();
        let reply_to = match &row.reply_to_id {
            Some(qid) => self.message_row(qid)?.map(|q| QuotedMessage {
                id: q.id.clone(),
                sender_name: if q.sender_account_id == me { "You".into() } else { self.member_name(&row.conversation_id, &q.sender_account_id) },
                kind: q.kind.clone(),
                text: if q.deleted { None } else { q.body.clone() },
                thumbnail: if q.deleted || q.view_once != 0 { None } else { q.attachments.first().and_then(|a| a.thumbnail.clone()) },
            }),
            None => None,
        };
        let reactions = {
            let db = self.db.borrow();
            let mut st = db.conn.prepare("SELECT account_id, emoji FROM reactions WHERE message_id = ?1 ORDER BY ts")?;
            let rows = st.query_map([&row.id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let reactions = reactions
            .into_iter()
            .map(|(account_id, emoji)| Reaction {
                is_me: account_id == me,
                sender_name: if account_id == me { "You".into() } else { self.member_name(&row.conversation_id, &account_id) },
                account_id,
                emoji,
            })
            .collect();
        let system = row.system.as_ref().map(|s| SystemEvent {
            kind: s.kind.clone(),
            actor_name: s.actor.as_ref().map(|a| if *a == me { "You".into() } else { self.member_name(&row.conversation_id, a) }),
            target_names: s
                .targets
                .iter()
                .map(|a| if *a == me { "You".into() } else { self.member_name(&row.conversation_id, a) })
                .collect(),
            value: s.value.clone(),
        });
        let marks = if row.deleted { Default::default() } else { self.message_marks(&row.id) };
        // Message requests: the sender's inline thumbnails are not handed to the UI to decode
        // (engine/requests.rs); the full image is there once the user downloads it.
        let request = !row.outgoing && !self.conversation_accepted_id(&row.conversation_id);
        // View-once media is shown only through `open_view_once` (engine/view_once.rs).
        let view_once = row.view_once != 0;
        Ok(Message {
            id: row.id.clone(),
            conversation_id: row.conversation_id.clone(),
            sender_account_id: row.sender_account_id.clone(),
            sender_name: if row.outgoing { "You".into() } else { self.member_name(&row.conversation_id, &row.sender_account_id) },
            outgoing: row.outgoing,
            kind: row.kind.clone(),
            text: if row.deleted { None } else { row.body.clone() },
            attachments: if row.deleted {
                vec![]
            } else {
                row.attachments
                    .iter()
                    .map(|a| Attachment {
                        thumbnail: if request || view_once { None } else { a.thumbnail.clone() },
                        local_path: if view_once { None } else { a.local_path.clone() },
                        ..Attachment::from(a)
                    })
                    .collect()
            },
            location: if row.deleted { None } else { row.location.clone() },
            contacts: if row.deleted { vec![] } else { row.contacts.clone() },
            reply_to,
            reactions,
            status: row.status.clone(),
            status_reason: if row.outgoing { row.status_reason.clone() } else { None },
            sent_at: row.sent_at,
            received_at: row.received_at,
            edited_at: row.edited_at,
            deleted: row.deleted,
            forwarded: row.forwarded,
            expires_at: row.expires_at,
            system,
            mentions_me: !row.deleted && row.mentions.iter().any(|m| m.account_id == me),
            mentions: if row.deleted { vec![] } else { row.mentions.clone() },
            pinned_until: marks.pinned_until,
            starred: marks.starred_until.is_some(),
            view_once: super::view_once::state_label(row.view_once),
        })
    }

    pub fn get_message(&self, id: &str) -> Result<Message> {
        self.message_model(&self.require_message(id)?)
    }

    pub fn emit_message(&self, id: &str, is_new_incoming: bool) {
        match self.message_row(id) {
            Ok(Some(row)) => match self.message_model(&row) {
                Ok(message) => self.emit(
                    "message",
                    &serde_json::json!({
                        "conversation_id": row.conversation_id,
                        "message": message,
                        "is_new_incoming": is_new_incoming,
                    }),
                ),
                Err(e) => log::warn!("message model {id}: {e}"),
            },
            Ok(None) => {}
            Err(e) => log::warn!("message {id}: {e}"),
        }
    }

    pub fn set_message_status(&self, id: &str, status: &str) -> Result<()> {
        let Some(row) = self.message_row(id)? else { return Ok(()) };
        let allowed = match status {
            "failed" => row.status == "pending" || row.status == "sent",
            "pending" => row.status == "failed",
            _ => status_rank(status) > status_rank(&row.status),
        };
        if allowed {
            // A reason explains only the state it was set in: a final failure keeps it, any
            // other change (sent, or pending again after a manual retry) clears it.
            self.db.borrow().conn.execute(
                "UPDATE messages SET status = ?2, status_reason = CASE WHEN ?2 = 'failed' THEN status_reason END WHERE id = ?1",
                params![id, status],
            )?;
            self.emit_message(id, false);
            self.emit_conversation(&row.conversation_id);
            if status != "pending" {
                self.vault_put_message(id);
            }
        }
        Ok(())
    }

    /// Sets (or clears) the reason shown under an outgoing message, without changing its status.
    pub fn set_status_reason(&self, id: &str, reason: Option<&str>) -> Result<()> {
        let Some(row) = self.message_row(id)? else { return Ok(()) };
        if row.status_reason.as_deref() == reason {
            return Ok(());
        }
        self.db.borrow().conn.execute("UPDATE messages SET status_reason = ?2 WHERE id = ?1", params![id, reason])?;
        self.emit_message(id, false);
        self.emit_conversation(&row.conversation_id);
        Ok(())
    }

    pub fn mark_sent(&self, id: &str) -> Result<()> {
        self.set_message_status(id, "sent")?;
        self.after_view_once_sent(id)
    }

    /// Status of an outgoing message from its receipts (group: all current members).
    pub fn aggregate_status(&self, id: &str, conv_id: &str, show_read: bool) -> Result<String> {
        let conv = self.conversation_row(conv_id)?;
        let me = self.me()?;
        let recipients: Vec<String> = match (&conv.peer_account_id, &conv.group_id) {
            (Some(p), _) => vec![p.clone()],
            (None, Some(g)) => self.group_member_ids(g)?.into_iter().filter(|a| *a != me).collect(),
            _ => vec![],
        };
        if recipients.is_empty() {
            return Ok("sent".into());
        }
        let db = self.db.borrow();
        let mut delivered = 0;
        let mut read = 0;
        for r in &recipients {
            let row: Option<(Option<i64>, Option<i64>)> = db
                .conn
                .query_row(
                    "SELECT delivered_at, read_at FROM receipts WHERE message_id = ?1 AND account_id = ?2",
                    params![id, r],
                    |x| Ok((x.get(0)?, x.get(1)?)),
                )
                .optional()?;
            if let Some((d, rd)) = row {
                if d.is_some() {
                    delivered += 1;
                }
                if rd.is_some() {
                    read += 1;
                }
            }
        }
        Ok(if show_read && read == recipients.len() {
            "read".into()
        } else if delivered == recipients.len() {
            "delivered".into()
        } else {
            "sent".into()
        })
    }

    pub fn mark_deleted_for_everyone(&self, id: &str) -> Result<()> {
        if let Some(row) = self.message_row(id)? {
            self.remove_attachment_files(&row.id, &row.attachments);
        }
        self.drop_marks(id);
        let db = self.db.borrow();
        db.conn.execute(
            "UPDATE messages SET deleted = 1, body = NULL, attachments_json = '[]', location_json = NULL, contacts_json = NULL, mentions_json = NULL,
                edit_history_json = NULL WHERE id = ?1",
            [id],
        )?;
        db.conn.execute("DELETE FROM reactions WHERE message_id = ?1", [id])?;
        Ok(())
    }

    /// Deletes the local files of a message's attachments, unless another message (a
    /// forwarded copy) still uses them.
    fn remove_attachment_files(&self, message_id: &str, atts: &[StoredAttachment]) {
        for a in atts {
            if let Some(p) = &a.local_path {
                let path = std::path::Path::new(p);
                // Only delete files we own (inside our media directory).
                if self.is_own_media(path) && !self.media_shared(p, message_id) {
                    let _ = std::fs::remove_file(path);
                }
            }
        }
    }

    /// Deletes a message locally (no network).
    pub fn delete_message_local(&self, id: &str) -> Result<()> {
        if let Some(conv) = self.delete_message_quiet(id)? {
            self.emit_conversation(&conv);
        }
        Ok(())
    }

    /// Deletes many messages locally with one conversation update per conversation at the end:
    /// an update per message (a cleared chat can hold thousands) floods the UI with renders.
    pub(super) fn delete_messages_local(&self, ids: &[String]) -> Result<()> {
        let mut convs = std::collections::BTreeSet::new();
        let mut first_err = None;
        for id in ids {
            match self.delete_message_quiet(id) {
                Ok(Some(conv)) => {
                    convs.insert(conv);
                }
                Ok(None) => {}
                Err(e) => {
                    log::warn!("delete {id}: {e}");
                    first_err.get_or_insert(e);
                }
            }
        }
        for conv in convs {
            self.emit_conversation(&conv);
        }
        first_err.map_or(Ok(()), Err)
    }

    /// [`Self::delete_message_local`] without the conversation update; returns the
    /// conversation of the deleted message.
    fn delete_message_quiet(&self, id: &str) -> Result<Option<String>> {
        let Some(row) = self.message_row(id)? else { return Ok(None) };
        self.remove_attachment_files(&row.id, &row.attachments);
        self.drop_marks(id);
        {
            let db = self.db.borrow();
            db.conn.execute("DELETE FROM messages WHERE id = ?1", [id])?;
            db.conn.execute("DELETE FROM reactions WHERE message_id = ?1", [id])?;
            db.conn.execute("DELETE FROM receipts WHERE message_id = ?1", [id])?;
        }
        self.vault_delete(&format!("msg:{id}"));
        self.recount_unread(&row.conversation_id)?;
        self.emit(
            "message_deleted",
            &serde_json::json!({ "conversation_id": row.conversation_id, "message_id": id }),
        );
        Ok(Some(row.conversation_id))
    }

    /// Starts the disappearing timer of received messages that were just read (`filter` is a
    /// WHERE clause on messages with one parameter). Our own messages count from sending.
    pub fn start_expiry_timers(&self, filter: &str, param: &str) -> Result<()> {
        let n = self.db.borrow().conn.execute(
            &format!(
                "UPDATE messages SET expires_at = ?2 + expires_in * 1000
                  WHERE {filter} AND outgoing = 0 AND read_local = 1 AND expires_in > 0 AND expires_at IS NULL"
            ),
            params![param, now_ms()],
        )?;
        if n > 0 {
            self.expiry_notify.notify_one();
        }
        Ok(())
    }

    pub fn recount_unread(&self, conv_id: &str) -> Result<()> {
        self.db.borrow().conn.execute(
            "UPDATE conversations SET unread_count = (SELECT COUNT(*) FROM messages WHERE conversation_id = ?1 AND outgoing = 0 AND read_local = 0) WHERE id = ?1",
            [conv_id],
        )?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn insert_system_message(
        &self,
        conv_id: &str,
        kind: &str,
        actor: Option<String>,
        targets: Vec<String>,
        value: Option<String>,
        id: Option<String>,
        at: i64,
    ) -> Result<()> {
        let id = id.map(|i| format!("sys-{i}")).unwrap_or_else(new_id);
        let inserted = self.insert_message(NewMessage {
            id: id.clone(),
            conversation_id: conv_id.to_string(),
            sender_account_id: actor.clone().unwrap_or_default(),
            sender_device: 0,
            outgoing: false,
            body: None,
            attachments: vec![],
            location: None,
            contacts: vec![],
            mentions: vec![],
            reply_to_id: None,
            forwarded: false,
            status: "read".into(),
            sent_at: at,
            received_at: now_ms(),
            expires_in: 0,
            expires_at: None,
            system: Some(StoredSystemEvent { kind: kind.to_string(), actor, targets, value }),
        })?;
        if inserted {
            self.emit_message(&id, false);
            self.emit_conversation(conv_id);
            self.vault_put_message(&id);
        }
        Ok(())
    }

    // --- commands -----------------------------------------------------------------

    /// Messages of a conversation, newest first, in the order they reached this device
    /// (`received_at`, our clock), not by `sent_at`: that is the sender's clock, and a peer
    /// whose clock is a minute off would see their messages interleaved with ours out of
    /// order. `before` is a `received_at` cursor (the UI passes the oldest shown message's).
    pub fn get_messages(&self, conv_id: &str, before: Option<i64>, limit: Option<i64>) -> Result<Vec<Message>> {
        let limit = limit.unwrap_or(50).clamp(1, 500);
        let rows: Vec<MessageRow> = {
            let db = self.db.borrow();
            let mut st = db.conn.prepare(&format!(
                "SELECT {MESSAGE_COLUMNS} FROM messages WHERE conversation_id = ?1 AND received_at < ?2 ORDER BY received_at DESC, rowid DESC LIMIT ?3"
            ))?;
            let rows = st.query_map(params![conv_id, before.unwrap_or(i64::MAX), limit], message_from_row)?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        rows.iter().map(|r| self.message_model(r)).collect()
    }

    pub async fn send_message(self: &Rc<Self>, req: SendMessageRequest) -> Result<Message> {
        let conv = self.conversation_row(&req.conversation_id)?;
        self.check_can_send(&conv)?;
        // Replying to a message request accepts it.
        self.accept_on_send(&conv.id)?;
        // Mentions are ranges in the text as typed; trimming would shift them.
        let text = req.text.filter(|t| !t.trim().is_empty());
        let mentions = sanitize_mentions(text.as_deref(), &req.mentions);
        let text = if mentions.is_empty() { text.map(|t| t.trim().to_string()) } else { text };
        if text.as_ref().map(|t| t.chars().count() > MAX_TEXT).unwrap_or(false) {
            return Err(CoreError::invalid("message is too long"));
        }
        if req.attachments.len() > MAX_ATTACHMENTS {
            return Err(CoreError::invalid("too many attachments"));
        }
        if req.contact_ids.len() > MAX_SHARED_CONTACTS {
            return Err(CoreError::invalid("too many contacts"));
        }
        let contacts = req
            .contact_ids
            .iter()
            .map(|id| self.shared_contact(id))
            .collect::<Result<Vec<_>>>()?;
        if text.is_none() && req.attachments.is_empty() && req.location.is_none() && contacts.is_empty() {
            return Err(CoreError::invalid("empty message"));
        }
        if req.view_once
            && !super::view_once::valid_view_once_shape(
                text.is_some() || req.location.is_some() || !contacts.is_empty(),
                req.attachments.iter().map(|a| a.kind.as_str()),
            )
        {
            return Err(CoreError::invalid("view-once messages are a single photo, video or voice message without text"));
        }
        let id = new_id();
        let mut attachments = Vec::new();
        for (i, a) in req.attachments.into_iter().enumerate() {
            attachments.push(self.prepare_outgoing_attachment(&id, i, a).await?);
        }
        if let Some(r) = &req.reply_to {
            if !self.message_row(r)?.map(|m| m.conversation_id == conv.id).unwrap_or(false) {
                return Err(CoreError::invalid("quoted message not found"));
            }
        }
        let now = now_ms();
        let me = self.me()?;
        self.insert_message(NewMessage {
            id: id.clone(),
            conversation_id: conv.id.clone(),
            sender_account_id: me,
            sender_device: self.session()?.account.device_id,
            outgoing: true,
            body: text,
            attachments,
            location: req.location,
            contacts,
            mentions,
            reply_to_id: req.reply_to,
            forwarded: false,
            status: "pending".into(),
            sent_at: now,
            received_at: now,
            expires_in: conv.disappearing_seconds,
            expires_at: (conv.disappearing_seconds > 0).then(|| now + conv.disappearing_seconds * 1000),
            system: None,
        })?;
        if req.view_once {
            self.mark_view_once(&id)?;
        }
        self.db.borrow().conn.execute("UPDATE conversations SET draft = NULL WHERE id = ?1", [&conv.id])?;
        self.enqueue(&Job::Message { message_id: id.clone() })?;
        self.emit_conversation(&conv.id);
        self.get_message(&id)
    }

    /// Builds the wire message for a stored outgoing message, uploading attachments first.
    pub async fn build_data_message(&self, id: &str) -> Result<Option<(String, DataMessage)>> {
        let Some(row) = self.message_row(id)? else { return Ok(None) };
        if row.deleted {
            return Ok(None);
        }
        let pointers = self.upload_message_attachments(id).await?;
        Ok(Some((
            row.conversation_id.clone(),
            DataMessage {
                id: row.id.clone(),
                body: row.body.clone(),
                attachments: pointers,
                location: row.location.clone(),
                contacts: row.contacts.clone(),
                mentions: row.mentions.clone(),
                quote: row.reply_to_id.clone(),
                expire_timer: row.expires_in,
                forwarded: row.forwarded,
                view_once: row.view_once != 0,
                sent_at: row.sent_at,
                ..Default::default()
            },
        )))
    }

    pub fn retry_message(&self, id: &str) -> Result<Message> {
        let row = self.require_message(id)?;
        if !row.outgoing || row.status != "failed" {
            return Err(CoreError::invalid("message is not failed"));
        }
        self.set_message_status(id, "pending")?;
        self.enqueue(&Job::Message { message_id: id.to_string() })?;
        self.get_message(id)
    }

    fn modification(&self, conv_id: &str) -> DataMessage {
        let conv = self.conversation_row(conv_id).ok();
        DataMessage {
            id: new_id(),
            sent_at: now_ms(),
            expire_timer: conv.map(|c| c.disappearing_seconds).unwrap_or(0),
            ..Default::default()
        }
    }

    pub fn edit_message(&self, id: &str, text: &str, mentions: &[Mention]) -> Result<Message> {
        let row = self.require_message(id)?;
        let mentions = sanitize_mentions(Some(text), mentions);
        let text = if mentions.is_empty() { text.trim() } else { text };
        if !row.outgoing || row.deleted || row.kind == "system" || row.view_once != 0 {
            return Err(CoreError::invalid("only your own messages can be edited"));
        }
        if text.is_empty() || text.chars().count() > MAX_TEXT {
            return Err(CoreError::invalid("invalid text"));
        }
        if now_ms() - row.sent_at > EDIT_WINDOW_MS {
            return Err(CoreError::invalid("this message is too old to be edited"));
        }
        if self.edit_count(id)? >= super::edits::MAX_EDITS {
            return Err(CoreError::invalid("this message cannot be edited again"));
        }
        let now = now_ms();
        let mentions_json = (!mentions.is_empty()).then(|| serde_json::to_string(&mentions)).transpose()?;
        self.apply_edit(id, text, mentions_json, now)?;
        let mut dm = self.modification(&row.conversation_id);
        dm.edit = Some(EditAction { target_id: id.to_string(), body: text.to_string(), mentions });
        self.enqueue(&Job::Data { conversation_id: row.conversation_id.clone(), data: dm })?;
        self.emit_message(id, false);
        self.emit_conversation(&row.conversation_id);
        self.vault_put_message(id);
        self.get_message(id)
    }

    pub fn delete_message(&self, id: &str, for_everyone: bool) -> Result<()> {
        let row = self.require_message(id)?;
        if !for_everyone {
            return self.delete_message_local(id);
        }
        if !row.outgoing || row.kind == "system" {
            return Err(CoreError::invalid("only your own messages can be deleted for everyone"));
        }
        if now_ms() - row.sent_at > DELETE_WINDOW_MS {
            return Err(CoreError::invalid("this message is too old to be deleted for everyone"));
        }
        self.mark_deleted_for_everyone(id)?;
        let mut dm = self.modification(&row.conversation_id);
        dm.delete = Some(id.to_string());
        self.enqueue(&Job::Data { conversation_id: row.conversation_id.clone(), data: dm })?;
        self.emit_message(id, false);
        self.emit_conversation(&row.conversation_id);
        self.vault_put_message(id);
        Ok(())
    }

    pub fn react(&self, id: &str, emoji: Option<String>) -> Result<Message> {
        let row = self.require_message(id)?;
        if row.deleted || row.kind == "system" {
            return Err(CoreError::invalid("cannot react to this message"));
        }
        let emoji = emoji.filter(|e| !e.is_empty());
        if emoji.as_deref().is_some_and(|e| !crate::sanitize::is_single_emoji(e)) {
            return Err(CoreError::invalid("invalid reaction"));
        }
        let me = self.me()?;
        match &emoji {
            Some(e) => self.db.borrow().conn.execute(
                "INSERT OR REPLACE INTO reactions (message_id, account_id, emoji, ts) VALUES (?1, ?2, ?3, ?4)",
                params![id, me, e, now_ms()],
            )?,
            None => self
                .db
                .borrow()
                .conn
                .execute("DELETE FROM reactions WHERE message_id = ?1 AND account_id = ?2", params![id, me])?,
        };
        let mut dm = self.modification(&row.conversation_id);
        dm.reaction = Some(ReactionAction { target_id: id.to_string(), emoji });
        self.enqueue(&Job::Data { conversation_id: row.conversation_id.clone(), data: dm })?;
        self.emit_message(id, false);
        self.vault_put_message(id);
        self.get_message(id)
    }

    pub fn forward_messages(&self, ids: &[String], conv_ids: &[String]) -> Result<()> {
        let me = self.me()?;
        let device = self.session()?.account.device_id;
        let mut rows = Vec::new();
        for id in ids {
            let r = self.require_message(id)?;
            if r.view_once != 0 {
                return Err(CoreError::invalid("view-once media cannot be forwarded"));
            }
            if self.message_media_expired(&r) {
                return Err(super::retention::expired_error());
            }
            if !r.deleted && r.kind != "system" {
                rows.push(r);
            }
        }
        rows.sort_by_key(|r| r.sent_at);
        for conv_id in conv_ids {
            let conv = self.conversation_row(conv_id)?;
            self.check_can_send(&conv)?;
            for r in &rows {
                let now = now_ms();
                let id = new_id();
                let attachments = r
                    .attachments
                    .iter()
                    .enumerate()
                    .map(|(i, a)| StoredAttachment { id: format!("{id}-{i}"), ..a.clone() })
                    .collect();
                self.insert_message(NewMessage {
                    id: id.clone(),
                    conversation_id: conv.id.clone(),
                    sender_account_id: me.clone(),
                    sender_device: device,
                    outgoing: true,
                    body: r.body.clone(),
                    attachments,
                    location: r.location.clone(),
                    contacts: r.contacts.clone(),
                    // Mentions name members of the original conversation: dropped.
                    mentions: vec![],
                    reply_to_id: None,
                    forwarded: true,
                    status: "pending".into(),
                    sent_at: now,
                    received_at: now,
                    expires_in: conv.disappearing_seconds,
                    expires_at: (conv.disappearing_seconds > 0).then(|| now + conv.disappearing_seconds * 1000),
                    system: None,
                })?;
                self.enqueue(&Job::Message { message_id: id.clone() })?;
                self.emit_message(&id, false);
            }
            self.emit_conversation(&conv.id);
        }
        Ok(())
    }

    pub fn mark_read(&self, conv_id: &str) -> Result<()> {
        let unread: Vec<(String, String)> = {
            let db = self.db.borrow();
            let mut st = db.conn.prepare(
                "SELECT id, sender_account_id FROM messages WHERE conversation_id = ?1 AND outgoing = 0 AND read_local = 0 AND kind != 'system'",
            )?;
            let rows = st.query_map([conv_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        {
            let db = self.db.borrow();
            db.conn.execute("UPDATE messages SET read_local = 1 WHERE conversation_id = ?1 AND read_local = 0", [conv_id])?;
            db.conn.execute("UPDATE conversations SET unread_count = 0 WHERE id = ?1", [conv_id])?;
        }
        self.start_expiry_timers("conversation_id = ?1", conv_id)?;
        self.emit_conversation(conv_id);
        if unread.is_empty() {
            return Ok(());
        }
        // No read receipts for message requests (sent when accepted, engine/requests.rs).
        if self.privacy()?.read_receipts && self.conversation_accepted_id(conv_id) {
            let mut by_sender: BTreeMap<String, Vec<String>> = BTreeMap::new();
            for (id, sender) in &unread {
                by_sender.entry(sender.clone()).or_default().push(id.clone());
            }
            for (sender, ids) in by_sender {
                if self.is_blocked(&sender) {
                    continue;
                }
                for chunk in ids.chunks(200) {
                    self.enqueue(&Job::Account {
                        account_id: sender.clone(),
                        content: Content::Receipt { kind: ReceiptKind::Read, ids: chunk.to_vec() },
                    })?;
                }
            }
        }
        for chunk in unread.chunks(200) {
            let entries = chunk.iter().map(|(id, s)| (s.clone(), id.clone())).collect();
            self.enqueue(&Job::OwnDevices { content: Content::Sync(SyncMessage::Read { entries }) })?;
        }
        Ok(())
    }

    pub fn message_info(&self, id: &str) -> Result<MessageInfo> {
        let row = self.require_message(id)?;
        let message = self.message_model(&row)?;
        let conv = self.conversation_row(&row.conversation_id)?;
        let me = self.me()?;
        let recipients: Vec<String> = match (&conv.peer_account_id, &conv.group_id) {
            (Some(p), _) => vec![p.clone()],
            (None, Some(g)) => self.group_member_ids(g)?.into_iter().filter(|a| *a != me).collect(),
            _ => vec![],
        };
        let mut receipts = Vec::new();
        for account in recipients {
            let r: Option<(Option<i64>, Option<i64>)> = self
                .db
                .borrow()
                .conn
                .query_row(
                    "SELECT delivered_at, read_at FROM receipts WHERE message_id = ?1 AND account_id = ?2",
                    params![id, account],
                    |x| Ok((x.get(0)?, x.get(1)?)),
                )
                .optional()?;
            let (delivered_at, read_at) = r.unwrap_or((None, None));
            let contact = self.contact_model(&account)?;
            receipts.push(MessageReceipt {
                status: if read_at.is_some() {
                    "read".into()
                } else if delivered_at.is_some() {
                    "delivered".into()
                } else {
                    "sent".into()
                },
                account_id: account,
                display_name: contact.display_name,
                avatar_path: contact.avatar_path,
                delivered_at,
                read_at,
            });
        }
        Ok(MessageInfo { message, receipts })
    }

    pub fn search_messages(&self, query: &str, conv_id: Option<&str>, limit: Option<i64>) -> Result<Vec<Message>> {
        let q = query.trim();
        if q.is_empty() {
            return Ok(vec![]);
        }
        let pattern = format!("%{}%", q.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
        let limit = limit.unwrap_or(100).clamp(1, 500);
        let rows: Vec<MessageRow> = {
            let db = self.db.borrow();
            let mut st = db.conn.prepare(&format!(
                "SELECT {MESSAGE_COLUMNS} FROM messages WHERE deleted = 0 AND body LIKE ?1 ESCAPE '\\'
                 AND (?2 IS NULL OR conversation_id = ?2) ORDER BY sent_at DESC LIMIT ?3"
            ))?;
            let rows = st.query_map(params![pattern, conv_id, limit], message_from_row)?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        rows.iter().map(|r| self.message_model(r)).collect()
    }

    pub fn list_media(&self, conv_id: &str, kind: &str, limit: Option<i64>) -> Result<Vec<Message>> {
        let kinds = if kind == "files" { "('file')" } else { "('image','video')" };
        let limit = limit.unwrap_or(200).clamp(1, 1000);
        let rows: Vec<MessageRow> = {
            let db = self.db.borrow();
            let mut st = db.conn.prepare(&format!(
                "SELECT {MESSAGE_COLUMNS} FROM messages WHERE conversation_id = ?1 AND deleted = 0 AND view_once = 0 AND kind IN {kinds}
                   AND instr(attachments_json, '\"expired_at\"') = 0
                 ORDER BY sent_at DESC LIMIT ?2"
            ))?;
            let rows = st.query_map(params![conv_id, limit], message_from_row)?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        rows.iter().map(|r| self.message_model(r)).collect()
    }

    /// Deletes expired disappearing messages.
    /// Applies a disappearing-messages timer to the messages already in the conversation too
    /// (not only to new ones): each one expires `seconds` after it was sent, so messages older
    /// than that go at once. Turning the timer off keeps the deadlines already set.
    pub(super) fn expire_existing(&self, conv_id: &str, seconds: i64) -> Result<()> {
        if seconds <= 0 {
            return Ok(());
        }
        let changed = self.db.borrow().conn.execute(
            "UPDATE messages SET expires_in = ?2, expires_at = sent_at + ?2 * 1000
             WHERE conversation_id = ?1 AND deleted = 0 AND (expires_at IS NULL OR expires_at > sent_at + ?2 * 1000)",
            params![conv_id, seconds],
        )?;
        if changed > 0 {
            self.expiry_notify.notify_one();
        }
        Ok(())
    }

    pub(super) async fn expiry_loop(self: Rc<Self>) {
        // The 30-day media retention runs here too: at start, then when due (retention.rs).
        let mut retention_due = 0;
        let mut retention_limit = self.media_retention_limit_ms();
        loop {
            // A changed period (here or synced from another device) is applied at once.
            if now_ms() >= retention_due || self.media_retention_limit_ms() != retention_limit {
                retention_limit = self.media_retention_limit_ms();
                retention_due = self.media_retention_tick().await;
            }
            let now = now_ms();
            let expired: Vec<String> = self
                .db
                .borrow()
                .conn
                .prepare("SELECT id FROM messages WHERE expires_at IS NOT NULL AND expires_at <= ?1")
                .and_then(|mut st| st.query_map([now], |r| r.get(0))?.collect())
                .unwrap_or_default();
            let purged = !expired.is_empty();
            // Errors are logged per message.
            let _ = self.delete_messages_local(&expired);
            if purged {
                // secure_delete zeroes the freed pages in the database file, but the write-ahead
                // log still holds copies of them until it is checkpointed: fold it in and
                // truncate it now, so no trace of the expired messages is left on disk.
                if let Err(e) = self.db.borrow().conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);") {
                    log::warn!("wal checkpoint after expiry: {e}");
                }
            }
            let next_mark = self.expire_marks();
            let next: Option<i64> = self
                .db
                .borrow()
                .conn
                .query_row("SELECT MIN(expires_at) FROM messages WHERE expires_at IS NOT NULL", [], |r| r.get(0))
                .unwrap_or(None);
            let next = match (next, next_mark) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            };
            let next = Some(next.map_or(retention_due, |t| t.min(retention_due)));
            let wait = next.map(|t| (t - now_ms()).clamp(200, 30_000)).unwrap_or(30_000);
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(wait as u64)) => {}
                _ = self.expiry_notify.notified() => {}
            }
        }
    }

    /// Warns about devices refused because their identity key does not match the account's
    /// pinned key (see store.rs): a red notice in the conversation (at most one per account
    /// and hour) and an `identity_conflict` event.
    pub fn process_identity_conflicts(&self) -> Result<()> {
        let conflicts: HashSet<String> = self.identity_conflicts.borrow_mut().drain(..).collect();
        let me = self.me()?;
        let hour = now_ms() / 3_600_000;
        for account in conflicts {
            let own = account == me;
            let conversation_id = if own { None } else { self.conversation_id_for_peer(&account)? };
            if let Some(conv) = &conversation_id {
                self.insert_system_message(
                    conv,
                    "identity_mismatch",
                    Some(account.clone()),
                    vec![],
                    None,
                    Some(format!("idmismatch-{account}-{hour}")),
                    now_ms(),
                )?;
            }
            self.emit(
                "identity_conflict",
                &serde_json::json!({ "account_id": account, "own_account": own, "conversation_id": conversation_id }),
            );
        }
        Ok(())
    }

    pub fn check_can_send(&self, conv: &super::conversations::ConversationRow) -> Result<()> {
        if let Some(peer) = &conv.peer_account_id {
            if self.is_blocked(peer) {
                return Err(CoreError::new(ErrorCode::Forbidden, "unblock this contact to send messages"));
            }
        }
        if let Some(g) = &conv.group_id {
            let (left, only_admins, member_names): (bool, bool, String) = self.db.borrow().conn.query_row(
                "SELECT left_group, only_admins_send, member_names FROM groups WHERE group_id = ?1",
                [g],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?;
            if left {
                return Err(CoreError::new(ErrorCode::Forbidden, "you are no longer a member of this group"));
            }
            if only_admins && self.my_group_role(g)?.as_deref() != Some("admin") {
                return Err(CoreError::new(ErrorCode::Forbidden, "only admins can send messages to this group"));
            }
            self.require_profile_name_for(&member_names)?;
        }
        Ok(())
    }
}

/// Most mentions kept in one message.
const MAX_MENTIONS: usize = 50;

/// Keeps well-formed mentions: inside the text (UTF-16 ranges), not overlapping, valid ids.
pub fn sanitize_mentions(text: Option<&str>, mentions: &[Mention]) -> Vec<Mention> {
    let Some(text) = text else { return vec![] };
    let len = text.encode_utf16().count() as u32;
    let mut sorted: Vec<&Mention> = mentions.iter().filter(|m| m.length > 0 && m.start.saturating_add(m.length) <= len).collect();
    sorted.sort_by_key(|m| m.start);
    let mut out: Vec<Mention> = Vec::new();
    for m in sorted {
        if out.last().is_some_and(|p| p.start + p.length > m.start) || uuid::Uuid::parse_str(&m.account_id).is_err() {
            continue;
        }
        out.push(m.clone());
        if out.len() == MAX_MENTIONS {
            break;
        }
    }
    out
}
