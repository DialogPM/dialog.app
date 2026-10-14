//! Conversations (direct and group chats).

use std::rc::Rc;

use rusqlite::{params, OptionalExtension};

use super::Inner;
use crate::content::DataMessage;
use crate::error::{CoreError, Result};
use crate::model::*;
use crate::util::{format_number, new_id, now_ms};

#[derive(Debug, Clone)]
pub struct ConversationRow {
    pub id: String,
    pub kind: String,
    pub peer_account_id: Option<String>,
    pub group_id: Option<String>,
    pub muted_until: Option<i64>,
    pub archived: bool,
    pub pinned: bool,
    pub disappearing_seconds: i64,
    pub draft: Option<String>,
    pub unread_count: i64,
    pub hidden: bool,
    pub updated_at: i64,
    /// Stored acceptance (see `conversation_accepted` for the full rule).
    pub accepted: bool,
}

const CONV_COLUMNS: &str = "id, kind, peer_account_id, group_id, muted_until, archived, pinned, \
    disappearing_seconds, draft, unread_count, hidden, updated_at, accepted";

fn conv_from_row(r: &rusqlite::Row) -> rusqlite::Result<ConversationRow> {
    Ok(ConversationRow {
        id: r.get(0)?,
        kind: r.get(1)?,
        peer_account_id: r.get(2)?,
        group_id: r.get(3)?,
        muted_until: r.get(4)?,
        archived: r.get(5)?,
        pinned: r.get(6)?,
        disappearing_seconds: r.get(7)?,
        draft: r.get(8)?,
        unread_count: r.get(9)?,
        hidden: r.get(10)?,
        updated_at: r.get(11)?,
        accepted: r.get(12)?,
    })
}

impl Inner {
    pub fn conversation_row(&self, id: &str) -> Result<ConversationRow> {
        self.db
            .borrow()
            .conn
            .query_row(&format!("SELECT {CONV_COLUMNS} FROM conversations WHERE id = ?1"), [id], conv_from_row)
            .optional()?
            .ok_or_else(|| CoreError::not_found("conversation not found"))
    }

    pub fn conversation_id_for_peer(&self, account_id: &str) -> Result<Option<String>> {
        Ok(self
            .db
            .borrow()
            .conn
            .query_row("SELECT id FROM conversations WHERE peer_account_id = ?1", [account_id], |r| r.get(0))
            .optional()?)
    }

    pub fn conversation_id_for_group(&self, group_id: &str) -> Result<Option<String>> {
        Ok(self
            .db
            .borrow()
            .conn
            .query_row("SELECT id FROM conversations WHERE group_id = ?1", [group_id], |r| r.get(0))
            .optional()?)
    }

    /// Returns the direct conversation with `account_id`, creating contact and conversation if needed.
    /// `accepted`: we start it ourselves (a new conversation started by the peer is a message
    /// request); an existing conversation keeps its state.
    pub async fn ensure_direct_conversation(&self, account_id: &str, accepted: bool) -> Result<String> {
        Ok(self.ensure_direct_conversation_new(account_id, accepted).await?.0)
    }

    /// `ensure_direct_conversation`, also telling whether the conversation was created now.
    async fn ensure_direct_conversation_new(&self, account_id: &str, accepted: bool) -> Result<(String, bool)> {
        if let Some(id) = self.conversation_id_for_peer(account_id)? {
            return Ok((id, false));
        }
        self.ensure_contact(account_id).await?;
        let id = new_id();
        let now = now_ms();
        self.db.borrow().conn.execute(
            "INSERT OR IGNORE INTO conversations (id, kind, peer_account_id, created_at, updated_at, accepted) VALUES (?1, 'direct', ?2, ?3, ?3, ?4)",
            params![id, account_id, now, accepted],
        )?;
        let created = match self.conversation_id_for_peer(account_id)? {
            Some(existing) if existing != id => return Ok((existing, false)),
            _ => true,
        };
        if accepted {
            self.send_presence_subscription();
        }
        // The timer of an earlier, deleted chat with them. Set quietly here (the peer started
        // it: each message carries its own timer); open_direct_conversation announces it.
        if let Some(t) = self.kept_chat_timer(account_id).filter(|t| *t > 0 && !accepted) {
            self.db.borrow().conn.execute("UPDATE conversations SET disappearing_seconds = ?2 WHERE id = ?1", params![id, t])?;
        }
        self.vault_put_conversation(&id);
        Ok((id, created))
    }

    /// Returns the group's conversation, creating it as a pending invite when new (accepted by
    /// whoever knows better: we created or joined it, a trusted member added us, the user joined).
    pub fn create_group_conversation(&self, group_id: &str) -> Result<String> {
        if let Some(id) = self.conversation_id_for_group(group_id)? {
            return Ok(id);
        }
        let id = new_id();
        let now = now_ms();
        self.db.borrow().conn.execute(
            "INSERT OR IGNORE INTO conversations (id, kind, group_id, created_at, updated_at, accepted) VALUES (?1, 'group', ?2, ?3, ?3, 0)",
            params![id, group_id, now],
        )?;
        Ok(self.conversation_id_for_group(group_id)?.unwrap_or(id))
    }

    pub fn conversation_model(&self, row: &ConversationRow) -> Result<Conversation> {
        let me = self.me().unwrap_or_default();
        let last = {
            let db = self.db.borrow();
            db.conn
                .query_row(
                    &format!(
                        "SELECT {} FROM messages WHERE conversation_id = ?1 ORDER BY received_at DESC, rowid DESC LIMIT 1",
                        super::messages::MESSAGE_COLUMNS
                    ),
                    [&row.id],
                    super::messages::message_from_row,
                )
                .optional()?
        };
        let last_message = last.map(|m| {
            let system = if m.kind == "system" { self.message_model(&m).ok().and_then(|mm| mm.system) } else { None };
            // A compact preview; the UI renders system events from `system`.
            let text = if m.kind == "system" { system.as_ref().and_then(|s| s.actor_name.clone()) } else { m.body.clone() };
            MessagePreview {
                id: m.id.clone(),
                kind: m.kind.clone(),
                text: if m.deleted { None } else { text },
                sender_name: if m.outgoing {
                    Some("You".into())
                } else if m.kind == "system" {
                    None
                } else {
                    Some(self.member_name(&row.id, &m.sender_account_id))
                },
                outgoing: m.outgoing,
                status: m.status.clone(),
                sent_at: m.sent_at,
                deleted: m.deleted,
                system,
                view_once: m.view_once != 0,
                media_expired: !m.deleted && m.attachments.first().is_some_and(super::retention::is_expired),
            }
        });
        let mut conv = Conversation {
            id: row.id.clone(),
            kind: row.kind.clone(),
            title: String::new(),
            avatar_path: None,
            peer_account_id: row.peer_account_id.clone(),
            peer_number: None,
            group_id: row.group_id.clone(),
            member_count: None,
            is_admin: false,
            left: false,
            last_message,
            unread_count: row.unread_count,
            unread_mentions: if row.unread_count > 0 { self.unread_mentions(&row.id, &me) } else { 0 },
            muted_until: row.muted_until.filter(|m| *m == -1 || *m > now_ms()),
            archived: row.archived,
            pinned: row.pinned,
            disappearing_seconds: row.disappearing_seconds,
            blocked: false,
            draft: row.draft.clone(),
            updated_at: row.updated_at,
            accepted: self.conversation_accepted(row),
            peer_is_contact: row.peer_account_id.as_deref().is_some_and(|p| self.is_contact(p)),
        };
        if let Some(peer) = &row.peer_account_id {
            let c = self.contact_model(peer)?;
            conv.title = if *peer == me { format!("{} (You)", c.display_name) } else { c.display_name };
            conv.avatar_path = c.avatar_path;
            conv.peer_number = Some(c.number);
            conv.blocked = c.blocked;
        } else if let Some(g) = &row.group_id {
            let info: Option<(String, Option<String>, String, bool)> = self
                .db
                .borrow()
                .conn
                .query_row(
                    "SELECT name, avatar_path, members_json, left_group FROM groups WHERE group_id = ?1",
                    [g],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .optional()?;
            if let Some((name, avatar, members_json, left)) = info {
                let members: Vec<super::groups::StoredMember> = serde_json::from_str(&members_json).unwrap_or_default();
                conv.title = if name.is_empty() { "Group".into() } else { name };
                conv.avatar_path = avatar;
                conv.member_count = Some(members.len() as i64);
                conv.is_admin = members.iter().any(|m| m.account_id == me && m.role == "admin");
                conv.left = left;
            } else {
                conv.title = "Group".into();
            }
        }
        Ok(conv)
    }

    pub fn get_conversation(&self, id: &str) -> Result<Conversation> {
        self.conversation_model(&self.conversation_row(id)?)
    }

    pub fn emit_conversation(&self, id: &str) {
        match self.get_conversation(id) {
            Ok(c) => self.emit("conversation_updated", &c),
            Err(e) => log::debug!("conversation {id}: {e}"),
        }
    }

    pub fn list_conversations(&self) -> Result<Vec<Conversation>> {
        let rows: Vec<ConversationRow> = {
            let db = self.db.borrow();
            let mut st = db.conn.prepare(&format!(
                "SELECT {CONV_COLUMNS} FROM conversations WHERE hidden = 0 ORDER BY pinned DESC, updated_at DESC"
            ))?;
            let rows = st.query_map([], conv_from_row)?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        rows.iter().map(|r| self.conversation_model(r)).collect()
    }

    /// Opens (starting it if needed) the direct conversation with `account_id`. A conversation
    /// started now gets the timer of an earlier, deleted chat with them, else the default
    /// disappearing timer (engine/default_timer.rs).
    pub async fn open_direct_conversation(&self, account_id: &str) -> Result<Conversation> {
        let (id, created) = self.ensure_direct_conversation_new(account_id, true).await?;
        self.db.borrow().conn.execute("UPDATE conversations SET hidden = 0 WHERE id = ?1", [&id])?;
        let timer = self.kept_chat_timer(account_id).unwrap_or_else(|| self.default_disappearing_seconds());
        if created && timer > 0 {
            let row = self.conversation_row(&id)?;
            // Not when we cannot send there (a blocked contact): the chat just starts without it.
            if self.check_can_send(&row).is_ok() {
                self.set_direct_timer(&row, timer)?;
                self.vault_put_conversation(&id);
            }
        }
        let c = self.get_conversation(&id)?;
        self.emit("conversation_updated", &c);
        Ok(c)
    }

    pub async fn set_conversation_settings(self: &Rc<Self>, patch: ConversationSettingsPatch) -> Result<Conversation> {
        let row = self.conversation_row(&patch.conversation_id)?;
        {
            let db = self.db.borrow();
            if let Some(m) = patch.muted_until {
                db.conn.execute("UPDATE conversations SET muted_until = ?2 WHERE id = ?1", params![row.id, m])?;
            }
            if let Some(a) = patch.archived {
                db.conn.execute("UPDATE conversations SET archived = ?2 WHERE id = ?1", params![row.id, a])?;
            }
            if let Some(p) = patch.pinned {
                db.conn.execute("UPDATE conversations SET pinned = ?2 WHERE id = ?1", params![row.id, p])?;
            }
        }
        if let (Some(d), Some(_)) = (patch.disappearing_seconds, &row.group_id) {
            // Groups: the timer is a group rule (admins only), applied to every member.
            if d != row.disappearing_seconds {
                self.update_group(UpdateGroupRequest {
                    conversation_id: row.id.clone(),
                    name: None,
                    description: None,
                    avatar_path: None,
                    settings: Some(GroupSettingsPatch { disappearing_seconds: Some(d), ..Default::default() }),
                })
                .await?;
            }
        } else if let Some(d) = patch.disappearing_seconds {
            let allowed = [0, 3600, 86400, 7 * 86400, 90 * 86400];
            if !allowed.contains(&d) && !(0..=365 * 86400).contains(&d) {
                return Err(CoreError::invalid("invalid disappearing timer"));
            }
            if d != row.disappearing_seconds {
                self.check_can_send(&row)?;
                self.set_direct_timer(&row, d)?;
            }
        }
        self.vault_put_conversation(&row.id);
        let c = self.get_conversation(&row.id)?;
        self.emit("conversation_updated", &c);
        Ok(c)
    }

    /// Sets a direct conversation's timer and announces it to the peer (timer update and a
    /// "disappearing_changed" system message); existing messages start expiring.
    fn set_direct_timer(&self, row: &ConversationRow, d: i64) -> Result<()> {
        self.db.borrow().conn.execute("UPDATE conversations SET disappearing_seconds = ?2 WHERE id = ?1", params![row.id, d])?;
        let now = now_ms();
        let dm = DataMessage { id: new_id(), sent_at: now, expire_timer: d, timer_update: true, ..Default::default() };
        self.insert_system_message(&row.id, "disappearing_changed", Some(self.me()?), vec![], Some(d.to_string()), Some(dm.id.clone()), now)?;
        self.expire_existing(&row.id, d)?;
        self.enqueue(&super::send::Job::Data { conversation_id: row.id.clone(), data: dm })
    }

    pub fn set_draft(&self, id: &str, draft: Option<String>) -> Result<()> {
        let draft = draft.filter(|d| !d.trim().is_empty());
        self.db.borrow().conn.execute("UPDATE conversations SET draft = ?2 WHERE id = ?1", params![id, draft])?;
        Ok(())
    }

    pub fn clear_conversation(&self, id: &str) -> Result<()> {
        let ids: Vec<String> = self
            .db
            .borrow()
            .conn
            .prepare("SELECT id FROM messages WHERE conversation_id = ?1")?
            .query_map([id], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        self.delete_messages_local(&ids)?;
        self.emit_conversation(id);
        Ok(())
    }

    /// Deletes a conversation from this device. A group we are still a member of is never
    /// removed (we would lose access to it): only its history is cleared.
    pub fn delete_conversation(&self, id: &str) -> Result<()> {
        self.clear_conversation(id)?;
        let row = self.conversation_row(id)?;
        if let Some(gid) = &row.group_id {
            if !self.group_left(gid) {
                return Ok(());
            }
            // Keep the row (the group may come back) but hide it.
            self.db.borrow().conn.execute("UPDATE conversations SET hidden = 1 WHERE id = ?1", [id])?;
        } else {
            self.keep_chat_acceptance(&row);
            self.keep_chat_timer(&row);
            self.db.borrow().conn.execute("DELETE FROM conversations WHERE id = ?1", [id])?;
            self.vault_delete(&format!("conv:{id}"));
        }
        self.emit("conversation_removed", &serde_json::json!({ "conversation_id": id }));
        Ok(())
    }

    /// Title fallback for accounts without any name.
    pub fn number_label(number: &str) -> String {
        if number.is_empty() {
            "Unknown".into()
        } else {
            format_number(number)
        }
    }

    /// Unread incoming messages that mention `me`.
    fn unread_mentions(&self, conv_id: &str, me: &str) -> i64 {
        self.db
            .borrow()
            .conn
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE conversation_id = ?1 AND outgoing = 0 AND read_local = 0 AND deleted = 0
                 AND mentions_json IS NOT NULL AND instr(mentions_json, ?2) > 0",
                rusqlite::params![conv_id, format!("\"{me}\"")],
                |r| r.get(0),
            )
            .unwrap_or(0)
    }
}
