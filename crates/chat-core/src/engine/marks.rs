//! Pinned and starred messages. Both are kept on this device only (never sent, never
//! backed up) and expire on their own: pins after the duration chosen when pinning, stars
//! after a week. They go away with their message (deleted, expired, cleared).

use rusqlite::{params, OptionalExtension};

use super::Inner;
use crate::error::{CoreError, Result};
use crate::model::Message;
use crate::util::now_ms;

/// Durations offered when pinning (24 hours, 7 days, 30 days).
pub const PIN_DURATIONS: [i64; 3] = [86_400, 7 * 86_400, 30 * 86_400];
/// At most this many pinned messages per conversation; pinning another unpins the oldest.
pub const MAX_PINS: usize = 3;
/// How long a star lasts.
pub const STAR_SECONDS: i64 = 7 * 86_400;

/// A message's pin / star state (only values still in the future).
#[derive(Debug, Clone, Copy, Default)]
pub struct Marks {
    pub pinned_until: Option<i64>,
    pub starred_until: Option<i64>,
}

impl Inner {
    pub(super) fn message_marks(&self, id: &str) -> Marks {
        let now = now_ms();
        self.db
            .borrow()
            .conn
            .query_row("SELECT pinned_until, starred_until FROM message_marks WHERE message_id = ?1", [id], |r| {
                Ok(Marks { pinned_until: r.get(0)?, starred_until: r.get(1)? })
            })
            .optional()
            .ok()
            .flatten()
            .map(|m| Marks {
                pinned_until: m.pinned_until.filter(|t| *t > now),
                starred_until: m.starred_until.filter(|t| *t > now),
            })
            .unwrap_or_default()
    }

    /// Pins a message for `seconds` (one of [`PIN_DURATIONS`]), or unpins it (`None`).
    pub fn pin_message(&self, id: &str, seconds: Option<i64>) -> Result<Message> {
        let row = self.message_row(id)?.ok_or_else(|| CoreError::not_found("message"))?;
        if let Some(s) = seconds {
            if !PIN_DURATIONS.contains(&s) {
                return Err(CoreError::invalid("unsupported pin duration"));
            }
            if row.deleted || row.system.is_some() {
                return Err(CoreError::invalid("this message cannot be pinned"));
            }
        }
        let now = now_ms();
        {
            let db = self.db.borrow();
            match seconds {
                Some(s) => {
                    db.conn.execute(
                        "INSERT INTO message_marks (message_id, conversation_id, pinned_at, pinned_until) VALUES (?1, ?2, ?3, ?4)
                         ON CONFLICT(message_id) DO UPDATE SET pinned_at = ?3, pinned_until = ?4",
                        params![id, row.conversation_id, now, now + s * 1000],
                    )?;
                }
                None => {
                    db.conn.execute("UPDATE message_marks SET pinned_at = NULL, pinned_until = NULL WHERE message_id = ?1", [id])?;
                }
            }
        }
        // Over the limit: the oldest pins go.
        let pinned = self.pinned_ids(&row.conversation_id)?;
        for old in pinned.iter().skip(MAX_PINS) {
            self.db.borrow().conn.execute("UPDATE message_marks SET pinned_at = NULL, pinned_until = NULL WHERE message_id = ?1", [old])?;
            self.emit_message(old, false);
        }
        self.cleanup_marks()?;
        self.expiry_notify.notify_one();
        self.emit_message(id, false);
        self.emit_pins_changed(&row.conversation_id);
        self.get_message(id)
    }

    /// Stars a message for a week, or removes the star.
    pub fn star_message(&self, id: &str, starred: bool) -> Result<Message> {
        let row = self.message_row(id)?.ok_or_else(|| CoreError::not_found("message"))?;
        if starred && (row.deleted || row.system.is_some()) {
            return Err(CoreError::invalid("this message cannot be starred"));
        }
        {
            let db = self.db.borrow();
            if starred {
                db.conn.execute(
                    "INSERT INTO message_marks (message_id, conversation_id, starred_until) VALUES (?1, ?2, ?3)
                     ON CONFLICT(message_id) DO UPDATE SET starred_until = ?3",
                    params![id, row.conversation_id, now_ms() + STAR_SECONDS * 1000],
                )?;
            } else {
                db.conn.execute("UPDATE message_marks SET starred_until = NULL WHERE message_id = ?1", [id])?;
            }
        }
        self.cleanup_marks()?;
        self.expiry_notify.notify_one();
        self.emit_message(id, false);
        self.get_message(id)
    }

    /// Ids of the conversation's pinned messages, most recently pinned first.
    fn pinned_ids(&self, conv_id: &str) -> Result<Vec<String>> {
        let db = self.db.borrow();
        let mut st = db.conn.prepare(
            "SELECT k.message_id FROM message_marks k JOIN messages m ON m.id = k.message_id
             WHERE k.conversation_id = ?1 AND k.pinned_until > ?2 AND m.deleted = 0
             ORDER BY k.pinned_at DESC, m.sent_at DESC, m.rowid DESC",
        )?;
        let ids = st.query_map(params![conv_id, now_ms()], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        Ok(ids)
    }

    /// The conversation's pinned messages, most recently pinned first.
    pub fn pinned_messages(&self, conv_id: &str) -> Result<Vec<Message>> {
        self.pinned_ids(conv_id)?.iter().map(|id| self.get_message(id)).collect()
    }

    /// Every starred message, newest first.
    pub fn starred_messages(&self) -> Result<Vec<Message>> {
        let ids: Vec<String> = {
            let db = self.db.borrow();
            let mut st = db.conn.prepare(
                "SELECT k.message_id FROM message_marks k JOIN messages m ON m.id = k.message_id
                 WHERE k.starred_until > ?1 AND m.deleted = 0 ORDER BY m.sent_at DESC, m.rowid DESC LIMIT 500",
            )?;
            let ids = st.query_map([now_ms()], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
            ids
        };
        ids.iter().map(|id| self.get_message(id)).collect()
    }

    /// Drops marks of messages that are gone or deleted, and marks that expired.
    pub(super) fn cleanup_marks(&self) -> Result<()> {
        let db = self.db.borrow();
        let now = now_ms();
        db.conn.execute("UPDATE message_marks SET pinned_at = NULL, pinned_until = NULL WHERE pinned_until <= ?1", [now])?;
        db.conn.execute("UPDATE message_marks SET starred_until = NULL WHERE starred_until <= ?1", [now])?;
        db.conn.execute(
            "DELETE FROM message_marks WHERE (pinned_until IS NULL AND starred_until IS NULL)
               OR message_id NOT IN (SELECT id FROM messages WHERE deleted = 0)",
            [],
        )?;
        Ok(())
    }

    /// Removes the marks of a message that is deleted (or deleted for everyone).
    pub(super) fn drop_marks(&self, id: &str) {
        let conv: Option<(String, Option<i64>)> = self
            .db
            .borrow()
            .conn
            .query_row("SELECT conversation_id, pinned_until FROM message_marks WHERE message_id = ?1", [id], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()
            .ok()
            .flatten();
        let Some((conv, pinned)) = conv else { return };
        let _ = self.db.borrow().conn.execute("DELETE FROM message_marks WHERE message_id = ?1", [id]);
        if pinned.is_some_and(|t| t > now_ms()) {
            self.emit_pins_changed(&conv);
        }
    }

    /// Expires pins and stars whose time is up; returns when the next one expires.
    pub(super) fn expire_marks(&self) -> Option<i64> {
        let now = now_ms();
        let due: Vec<(String, String, bool)> = self
            .db
            .borrow()
            .conn
            .prepare(
                "SELECT message_id, conversation_id, pinned_until IS NOT NULL AND pinned_until <= ?1 FROM message_marks
                 WHERE pinned_until <= ?1 OR starred_until <= ?1",
            )
            .and_then(|mut st| st.query_map([now], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect())
            .unwrap_or_default();
        if !due.is_empty() {
            if let Err(e) = self.cleanup_marks() {
                log::warn!("expire marks: {e}");
            }
            let mut convs = std::collections::HashSet::new();
            for (id, conv, unpinned) in due {
                self.emit_message(&id, false);
                if unpinned {
                    convs.insert(conv);
                }
            }
            for c in convs {
                self.emit_pins_changed(&c);
            }
        }
        self.db
            .borrow()
            .conn
            .query_row(
                "SELECT MIN(t) FROM (SELECT pinned_until AS t FROM message_marks WHERE pinned_until > ?1
                 UNION ALL SELECT starred_until FROM message_marks WHERE starred_until > ?1)",
                [now],
                |r| r.get(0),
            )
            .unwrap_or(None)
    }

    fn emit_pins_changed(&self, conv_id: &str) {
        self.emit("pins_changed", &serde_json::json!({ "conversation_id": conv_id }));
    }
}
