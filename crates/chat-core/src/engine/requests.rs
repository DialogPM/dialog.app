//! Message requests: conversations started by accounts we do not know.
//!
//! A direct conversation is accepted when the peer is in our contacts, when we started it or
//! wrote in it, or when the user accepted it. Deleting an accepted chat keeps the acceptance for
//! that person (`contacts.chat_accepted`): a new chat with them is not a request again. A group is accepted when we created or joined it
//! ourselves, when someone we trust (a contact or an accepted chat) added us, or when the user
//! joined it. Conversations that existed before this rule count as accepted.
//!
//! Until then the stranger gets nothing back: no delivery/read receipts, no typing indicators,
//! not our profile key (so neither our name and avatar, nor sealed-sender access, nor our
//! presence, which the server only shows to holders of our access key). On our side nothing
//! is fetched or decoded automatically: no attachment downloads, no avatar, no thumbnails, and
//! their calls do not ring (engine/calls.rs). Messages are still stored and shown.

use std::rc::Rc;

use rusqlite::{params, OptionalExtension};

use super::conversations::ConversationRow;
use super::send::Job;
use super::Inner;
use crate::content::{Content, ReceiptKind};
use crate::error::Result;
use crate::model::{Conversation, StoredAttachment};

/// Receipts sent on acceptance cover at most this many recent messages.
const ACCEPT_RECEIPTS_MAX: i64 = 500;

impl Inner {
    /// The account is in our contacts.
    pub fn is_contact(&self, account_id: &str) -> bool {
        self.db
            .borrow()
            .conn
            .query_row("SELECT is_contact FROM contacts WHERE account_id = ?1", [account_id], |r| r.get(0))
            .unwrap_or(false)
    }

    /// Whether what `account_id` starts (a chat, a group they add us to, a call) is accepted
    /// without asking: ourselves, a contact, or someone whose direct chat we accepted.
    pub fn trusts_account(&self, account_id: &str) -> bool {
        if self.me().is_ok_and(|me| me == account_id) || self.is_contact(account_id) || self.chat_accepted_before(account_id) {
            return true;
        }
        self.db
            .borrow()
            .conn
            .query_row(
                "SELECT accepted FROM conversations WHERE peer_account_id = ?1",
                [account_id],
                |r| r.get::<_, bool>(0),
            )
            .optional()
            .ok()
            .flatten()
            .unwrap_or(false)
    }

    /// We accepted a direct chat with the account that was deleted since.
    fn chat_accepted_before(&self, account_id: &str) -> bool {
        self.db
            .borrow()
            .conn
            .query_row("SELECT chat_accepted FROM contacts WHERE account_id = ?1 AND blocked = 0", [account_id], |r| r.get(0))
            .unwrap_or(false)
    }

    /// Before a direct conversation is deleted: remember that the chat with this person was
    /// accepted, so writing again (either side) does not make it a message request again.
    pub(super) fn keep_chat_acceptance(&self, row: &ConversationRow) {
        let Some(peer) = row.peer_account_id.as_deref() else { return };
        if !self.conversation_accepted(row) || self.is_blocked(peer) {
            return;
        }
        let _ = self.db.borrow().conn.execute("UPDATE contacts SET chat_accepted = 1 WHERE account_id = ?1", [peer]);
    }

    /// Disappearing timer of an earlier, deleted chat with the account (see `keep_chat_timer`).
    pub(super) fn kept_chat_timer(&self, account_id: &str) -> Option<i64> {
        self.db
            .borrow()
            .conn
            .query_row("SELECT chat_timer FROM contacts WHERE account_id = ?1", [account_id], |r| r.get(0))
            .ok()
            .flatten()
    }

    /// Before a direct conversation of an accepted chat is deleted: keep its disappearing timer
    /// (also "off"), applied again to the next chat with that person until the user changes it.
    pub(super) fn keep_chat_timer(&self, row: &ConversationRow) {
        let Some(peer) = row.peer_account_id.as_deref() else { return };
        if !self.conversation_accepted(row) {
            return;
        }
        let _ = self.db.borrow().conn.execute(
            "UPDATE contacts SET chat_timer = ?2 WHERE account_id = ?1",
            params![peer, row.disappearing_seconds],
        );
    }

    /// Whether we share an accepted conversation with the account (trusted, or a member of a
    /// group we accepted): their avatar may then be downloaded automatically.
    pub fn shares_accepted_chat(&self, account_id: &str) -> bool {
        if self.trusts_account(account_id) {
            return true;
        }
        self.db
            .borrow()
            .conn
            .query_row(
                "SELECT 1 FROM groups g JOIN conversations c ON c.group_id = g.group_id
                 WHERE c.accepted = 1 AND g.left_group = 0 AND instr(g.members_json, ?1) > 0 LIMIT 1",
                [format!("\"{account_id}\"")],
                |_| Ok(()),
            )
            .optional()
            .ok()
            .flatten()
            .is_some()
    }

    /// The full acceptance rule for a conversation (direct chats with contacts always are).
    pub fn conversation_accepted(&self, row: &ConversationRow) -> bool {
        row.accepted || row.peer_account_id.as_deref().is_some_and(|p| self.trusts_account(p))
    }

    pub fn conversation_accepted_id(&self, conv_id: &str) -> bool {
        self.conversation_row(conv_id).map(|r| self.conversation_accepted(&r)).unwrap_or(false)
    }

    /// Stores the acceptance. Returns whether it changed.
    fn set_accepted(&self, conv_id: &str) -> Result<bool> {
        let changed = self
            .db
            .borrow()
            .conn
            .execute("UPDATE conversations SET accepted = 1 WHERE id = ?1 AND accepted = 0", [conv_id])?
            > 0;
        if changed {
            self.vault_put_conversation(conv_id);
        }
        Ok(changed)
    }

    /// We wrote in the conversation (message, call): that accepts it.
    pub fn accept_on_send(self: &Rc<Self>, conv_id: &str) -> Result<()> {
        let Ok(row) = self.conversation_row(conv_id) else { return Ok(()) };
        if row.accepted {
            return Ok(());
        }
        let was = self.conversation_accepted(&row);
        if self.set_accepted(conv_id)? && !was {
            self.after_accept(&row)?;
        }
        Ok(())
    }

    /// Marks a conversation accepted without telling anyone: we created or joined the group
    /// (the group flow shares what is needed itself), a trusted member added us, or another
    /// of our devices accepted or wrote in it (and sent what was held back).
    /// Returns whether the conversation stopped being a request (held downloads can start).
    pub(super) fn mark_accepted(&self, conv_id: &str) -> Result<bool> {
        let was = self.conversation_accepted_id(conv_id);
        if !self.set_accepted(conv_id)? {
            return Ok(false);
        }
        if !was {
            self.send_presence_subscription();
        }
        self.emit_conversation(conv_id);
        Ok(!was)
    }

    /// The user accepted a message request or joined a group they were added to.
    pub async fn accept_conversation(self: &Rc<Self>, conv_id: &str) -> Result<Conversation> {
        let row = self.conversation_row(conv_id)?;
        let was = self.conversation_accepted(&row);
        if self.set_accepted(conv_id)? && !was {
            self.after_accept(&row)?;
            if let Some(gid) = row.group_id.clone() {
                // Fetch what was held back (the group's avatar).
                let me = self.clone();
                tokio::task::spawn_local(async move {
                    if let Err(e) = me.sync_group(&gid, None).await {
                        log::warn!("group sync {gid}: {e}");
                    }
                });
            }
        }
        let c = self.get_conversation(conv_id)?;
        self.emit("conversation_updated", &c);
        Ok(c)
    }

    /// Everything held back while the conversation was a request.
    fn after_accept(self: &Rc<Self>, row: &ConversationRow) -> Result<()> {
        let me = self.me()?;
        // Our profile key: the peer can now see our name and avatar, write to us sealed and
        // see our presence.
        if let Some(peer) = row.peer_account_id.as_deref().filter(|p| *p != me && !self.is_blocked(p)) {
            let key = self.profile_key_b64()?;
            self.enqueue(&Job::Account { account_id: peer.to_string(), content: Content::ProfileUpdated { profile_key: key } })?;
            self.send_presence_subscription();
            let (me2, peer) = (self.clone(), peer.to_string());
            tokio::task::spawn_local(async move {
                if let Err(e) = me2.fetch_profile(&peer).await {
                    log::debug!("profile {peer}: {e}");
                }
            });
        }
        if let Some(gid) = &row.group_id {
            self.share_profile_key_in_group(gid, None)?;
        }
        self.send_held_receipts(&row.id)?;
        self.download_held_attachments(&row.id)?;
        Ok(())
    }

    /// Delivery receipts (and read receipts, if enabled) for the messages received while the
    /// conversation was a request.
    fn send_held_receipts(&self, conv_id: &str) -> Result<()> {
        let rows: Vec<(String, String, bool)> = {
            let db = self.db.borrow();
            let mut st = db.conn.prepare(
                "SELECT id, sender_account_id, read_local FROM messages
                 WHERE conversation_id = ?1 AND outgoing = 0 AND kind != 'system' AND deleted = 0
                 ORDER BY sent_at DESC LIMIT ?2",
            )?;
            let rows = st.query_map(params![conv_id, ACCEPT_RECEIPTS_MAX], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let read_receipts = self.privacy()?.read_receipts;
        let mut by_sender: std::collections::BTreeMap<String, (Vec<String>, Vec<String>)> = Default::default();
        for (id, sender, read) in rows {
            let e = by_sender.entry(sender).or_default();
            if read && read_receipts {
                e.1.push(id);
            } else {
                e.0.push(id);
            }
        }
        for (sender, (delivered, read)) in by_sender {
            if self.is_blocked(&sender) {
                continue;
            }
            for (kind, ids) in [(ReceiptKind::Delivered, delivered), (ReceiptKind::Read, read)] {
                for chunk in ids.chunks(200) {
                    self.enqueue(&Job::Account {
                        account_id: sender.clone(),
                        content: Content::Receipt { kind, ids: chunk.to_vec() },
                    })?;
                }
            }
        }
        Ok(())
    }

    /// Starts the downloads that would have been automatic (images and voice notes).
    pub(super) fn download_held_attachments(self: &Rc<Self>, conv_id: &str) -> Result<()> {
        let rows: Vec<(String, String)> = {
            let db = self.db.borrow();
            let mut st = db.conn.prepare(
                "SELECT id, attachments_json FROM messages
                 WHERE conversation_id = ?1 AND outgoing = 0 AND deleted = 0 AND attachments_json != '[]'
                 ORDER BY sent_at DESC LIMIT 100",
            )?;
            let rows = st.query_map([conv_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        for (mid, json) in rows {
            let atts: Vec<StoredAttachment> = serde_json::from_str(&json).unwrap_or_default();
            for a in atts.into_iter().filter(|a| auto_downloadable(a) && a.download_state == "pending") {
                let (me, mid, aid) = (self.clone(), mid.clone(), a.id);
                tokio::task::spawn_local(async move {
                    if let Err(e) = me.download_attachment(&mid, &aid).await {
                        log::warn!("download {aid}: {e}");
                    }
                });
            }
        }
        Ok(())
    }
}

/// Attachments downloaded without asking in accepted conversations.
pub(super) fn auto_downloadable(a: &StoredAttachment) -> bool {
    (a.kind == "image" || a.kind == "voice") && a.size <= super::receive::AUTO_DOWNLOAD_MAX
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{Core, CoreConfig, EventSink};
    use std::sync::Arc;

    struct NoEvents;
    impl EventSink for NoEvents {
        fn emit(&self, _: &str, _: serde_json::Value) {}
    }

    fn core() -> (Core, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let core = Core::start(CoreConfig {
            data_dir: dir.path().to_path_buf(),
            db_key: [3u8; 32],
            default_server_url: String::new(),
            platform: "test".into(),
            events: Arc::new(NoEvents),
        })
        .unwrap();
        (core, dir)
    }

    const PEER: &str = "11111111-1111-4111-8111-111111111111";

    fn insert_direct(i: &Inner, id: &str, peer: &str, accepted: bool) {
        let db = i.db.borrow();
        db.conn
            .execute("INSERT INTO contacts (account_id, number, updated_at) VALUES (?1, '1234567812345678', 0)", [peer])
            .unwrap();
        db.conn
            .execute(
                "INSERT INTO conversations (id, kind, peer_account_id, created_at, updated_at, accepted) VALUES (?1, 'direct', ?2, 0, 0, ?3)",
                params![id, peer, accepted],
            )
            .unwrap();
    }

    #[tokio::test]
    async fn stranger_chat_is_a_request_until_contact_or_accepted() {
        let (core, _dir) = core();
        core.run(|i| async move {
            insert_direct(&i, "c1", PEER, false);
            assert!(!i.conversation_accepted_id("c1"));
            assert!(!i.trusts_account(PEER));
            // Adding the peer to the contacts accepts the chat.
            i.db.borrow().conn.execute("UPDATE contacts SET is_contact = 1 WHERE account_id = ?1", [PEER])?;
            assert!(i.conversation_accepted_id("c1"));
            assert!(i.trusts_account(PEER));
            i.db.borrow().conn.execute("UPDATE contacts SET is_contact = 0 WHERE account_id = ?1", [PEER])?;
            // An explicit acceptance is stored.
            assert!(i.set_accepted("c1")?);
            assert!(!i.set_accepted("c1")?, "accepting twice changes nothing");
            assert!(i.conversation_accepted_id("c1"));
            assert!(i.trusts_account(PEER));
            Ok(())
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn new_groups_start_as_invites_and_members_of_accepted_groups_are_known() {
        let (core, _dir) = core();
        core.run(|i| async move {
            let conv = i.create_group_conversation("g1")?;
            assert!(!i.conversation_accepted_id(&conv));
            i.db.borrow().conn.execute(
                "INSERT INTO groups (group_id, members_json, distribution_id) VALUES ('g1', ?1, 'd')",
                [format!(r#"[{{"account_id":"{PEER}","role":"admin"}}]"#)],
            )?;
            assert!(!i.shares_accepted_chat(PEER), "a pending group does not vouch for its members");
            i.mark_accepted(&conv)?;
            assert!(i.conversation_accepted_id(&conv));
            assert!(i.shares_accepted_chat(PEER));
            assert!(!i.trusts_account(PEER), "group members are not trusted to start chats");
            Ok(())
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn deleting_an_accepted_chat_keeps_it_accepted_for_that_person() {
        let (core, _dir) = core();
        core.run(|i| async move {
            insert_direct(&i, "c1", PEER, true);
            i.delete_conversation("c1")?;
            assert!(i.trusts_account(PEER), "a new chat with them is not a request");
            let conv = i.ensure_direct_conversation(PEER, false).await?;
            assert!(i.conversation_accepted_id(&conv));
            // Blocking ends it.
            i.db.borrow().conn.execute("UPDATE contacts SET blocked = 1 WHERE account_id = ?1", [PEER])?;
            assert!(!i.trusts_account(PEER));
            Ok(())
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn a_deleted_chats_timer_applies_to_the_next_chat_with_that_person() {
        let (core, _dir) = core();
        core.run(|i| async move {
            insert_direct(&i, "c1", PEER, true);
            i.db.borrow().conn.execute("UPDATE conversations SET disappearing_seconds = 10800 WHERE id = 'c1'", [])?;
            i.delete_conversation("c1")?;
            assert_eq!(i.kept_chat_timer(PEER), Some(10800));
            // The peer writes first: the new chat has the timer again.
            let conv = i.ensure_direct_conversation(PEER, false).await?;
            assert_eq!(i.conversation_row(&conv)?.disappearing_seconds, 10800);
            Ok(())
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn deleting_a_request_does_not_accept_it() {
        let (core, _dir) = core();
        core.run(|i| async move {
            insert_direct(&i, "c1", PEER, false);
            i.delete_conversation("c1")?;
            assert!(!i.trusts_account(PEER));
            Ok(())
        })
        .await
        .unwrap();
    }

    #[test]
    fn migration_keeps_existing_conversations_accepted() {
        let db = crate::db::Db::open_in_memory().unwrap();
        db.conn
            .execute("INSERT INTO conversations (id, kind, created_at, updated_at) VALUES ('x', 'direct', 0, 0)", [])
            .unwrap();
        let accepted: bool = db.conn.query_row("SELECT accepted FROM conversations WHERE id = 'x'", [], |r| r.get(0)).unwrap();
        assert!(accepted);
    }

    #[test]
    fn only_small_images_and_voice_download_automatically() {
        let a = |kind: &str, size: i64| StoredAttachment { kind: kind.into(), size, ..Default::default() };
        assert!(auto_downloadable(&a("image", 1000)));
        assert!(auto_downloadable(&a("voice", 1000)));
        assert!(!auto_downloadable(&a("file", 1000)));
        assert!(!auto_downloadable(&a("video", 1000)));
        assert!(!auto_downloadable(&a("image", 64 * 1024 * 1024)));
    }
}
