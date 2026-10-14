//! Deleting conversations on both sides, and wiping everything.
//!
//! "For both" sends an end-to-end encrypted command to the other side; the server stores it
//! like any message, so a peer that is offline applies it as soon as it comes online. Our own
//! other devices follow through the vault tombstones of the deleted messages.

use rusqlite::params;

use super::send::Job;
use super::Inner;
use crate::content::Content;
use crate::error::{CoreError, ErrorCode, Result};
use crate::util::now_ms;

impl Inner {
    /// Destructive actions (deleting for everyone, wiping, resetting): the account password
    /// confirms it is really the owner, not someone with a moment at an unlocked device.
    pub async fn require_password(&self, password: &str) -> Result<()> {
        if password.is_empty() || !self.verify_password(password).await? {
            return Err(CoreError::new(ErrorCode::InvalidPassword, "wrong password"));
        }
        Ok(())
    }

    /// `reset_local_data` from a host: without the password only while the data is locked
    /// (the "forgot password" path of the lock screen); once unlocked, the password is required.
    pub async fn reset_local_data_checked(&self, password: Option<&str>) -> Result<()> {
        if !self.locked.get() {
            self.require_password(password.unwrap_or_default()).await?;
        }
        self.reset_local_data()
    }

    fn conversation_message_ids(&self, conv_id: &str, sender: Option<&str>, before: i64) -> Result<Vec<String>> {
        let db = self.db.borrow();
        let mut st = db.conn.prepare(
            "SELECT id FROM messages WHERE conversation_id = ?1 AND sent_at <= ?3 AND (?2 IS NULL OR sender_account_id = ?2)",
        )?;
        let ids = st.query_map(params![conv_id, sender, before], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        Ok(ids)
    }

    /// Deletes a conversation here and asks the other side(s) to delete it too.
    /// Direct chat: the whole conversation on both sides. Group: only an admin can do it, and
    /// it clears the history for every member; the group itself stays.
    pub fn delete_conversation_for_everyone(&self, conv_id: &str) -> Result<()> {
        let row = self.conversation_row(conv_id)?;
        let before = now_ms();
        if row.group_id.is_some() {
            return self.clear_group_for_everyone(conv_id);
        } else if let Some(peer) = &row.peer_account_id {
            if *peer == self.me()? {
                return self.delete_conversation(conv_id);
            }
            self.enqueue(&Job::Account { account_id: peer.clone(), content: Content::ConversationWiped { before, peer: None } })?;
            // And on our own other devices, whichever device deleted it.
            let me = self.me()?;
            self.enqueue(&Job::Account { account_id: me, content: Content::ConversationWiped { before, peer: Some(peer.clone()) } })?;
        } else {
            return Err(CoreError::invalid("conversation has no target"));
        }
        self.delete_conversation(conv_id)
    }

    /// Deletes every conversation on this account, here and (queued) on the other side of
    /// each one, then compacts the database so deleted content does not linger in free pages.
    pub fn wipe_all_conversations(&self) -> Result<()> {
        let ids: Vec<String> = self
            .db
            .borrow()
            .conn
            .prepare("SELECT id FROM conversations")?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        for id in ids {
            // Groups: only admins clear them for everyone; members clear their own copy.
            let group = self.conversation_row(&id)?.group_id;
            if let Some(gid) = group {
                if !self.is_group_admin(&gid, &self.me()?) || self.clear_group_for_everyone(&id).is_err() {
                    self.delete_conversation(&id)?;
                }
                continue;
            }
            if let Err(e) = self.delete_conversation_for_everyone(&id) {
                log::warn!("wipe {id}: {e}");
                self.delete_conversation(&id)?;
            }
        }
        // Unsent messages of the deleted chats must not go out anymore.
        self.db.borrow().conn.execute("DELETE FROM outbox WHERE job LIKE '%\"k\":\"message\"%'", [])?;
        // Anything left in the media directory belonged to deleted messages.
        if let Ok(entries) = std::fs::read_dir(self.media_dir()) {
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if !name.starts_with("avatar-") {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
        self.compact_database()?;
        Ok(())
    }

    /// Rewrites the database file (VACUUM) and truncates the WAL, so freed pages that held
    /// deleted messages are gone from disk (secure_delete also zeroes them as they are freed).
    pub fn compact_database(&self) -> Result<()> {
        let db = self.db.borrow();
        db.conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); VACUUM; PRAGMA wal_checkpoint(TRUNCATE);")?;
        Ok(())
    }

    /// The peer deleted our direct conversation for both of us.
    pub(super) fn apply_conversation_wiped(&self, peer: &str, before: i64) -> Result<()> {
        let Some(conv) = self.conversation_id_for_peer(peer)? else { return Ok(()) };
        self.delete_messages_local(&self.conversation_message_ids(&conv, None, before)?)?;
        let left: i64 = self.db.borrow().conn.query_row("SELECT COUNT(*) FROM messages WHERE conversation_id = ?1", [&conv], |r| r.get(0))?;
        if left == 0 {
            self.delete_conversation(&conv)?;
        }
        log::info!("conversation with {peer} wiped by the peer");
        Ok(())
    }

    /// Clears the group history for every member (admins only). The group stays, with a
    /// notice of who cleared it.
    pub fn clear_group_for_everyone(&self, conv_id: &str) -> Result<()> {
        let row = self.conversation_row(conv_id)?;
        let gid = row.group_id.ok_or_else(|| CoreError::invalid("not a group"))?;
        let me = self.me()?;
        if self.group_left(&gid) || !self.is_group_admin(&gid, &me) {
            return Err(CoreError::new(ErrorCode::Forbidden, "only group admins can clear the history for everyone"));
        }
        let before = now_ms();
        self.enqueue(&Job::Group { group_id: gid.clone(), content: Content::GroupHistoryCleared { group_id: gid.clone(), before } })?;
        self.apply_group_history_cleared(conv_id, &me, before)
    }

    /// An admin cleared the group history (checked by the caller).
    pub(super) fn apply_group_history_cleared(&self, conv_id: &str, admin: &str, before: i64) -> Result<()> {
        let key = format!("history_cleared:{conv_id}");
        let prev: Option<i64> = self.db.borrow().kv_get_json(&key)?;
        self.db.borrow().kv_set_json(&key, &prev.unwrap_or(0).max(before))?;
        self.delete_messages_local(&self.conversation_message_ids(conv_id, None, before)?)?;
        self.insert_system_message(conv_id, "group_history_cleared", Some(admin.to_string()), vec![], None, None, before)?;
        self.emit_conversation(conv_id);
        Ok(())
    }

    /// An admin of the group cleared its history.
    pub(super) fn on_group_history_cleared(&self, group_id: &str, sender: &str, before: i64) -> Result<()> {
        if !self.is_group_admin(group_id, sender) {
            log::warn!("history clear for group {group_id} from a non-admin ignored");
            return Ok(());
        }
        let conv: Option<String> = self
            .db
            .borrow()
            .conn
            .query_row("SELECT id FROM conversations WHERE group_id = ?1", [group_id], |r| r.get(0))
            .ok();
        let Some(conv) = conv else { return Ok(()) };
        self.apply_group_history_cleared(&conv, sender, before)
    }

    /// A member deleted the messages they sent to the group (older clients).
    pub(super) fn apply_group_messages_wiped(&self, group_id: &str, sender: &str, before: i64) -> Result<()> {
        let conv: Option<String> = self
            .db
            .borrow()
            .conn
            .query_row("SELECT id FROM conversations WHERE group_id = ?1", [group_id], |r| r.get(0))
            .ok();
        let Some(conv) = conv else { return Ok(()) };
        self.delete_messages_local(&self.conversation_message_ids(&conv, Some(sender), before)?)?;
        Ok(())
    }
}
