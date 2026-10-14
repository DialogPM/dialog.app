//! Messages that could not be decrypted: a visible placeholder instead of silence, and
//! bounded answers to other people's resend requests (no amplification).

use rusqlite::{params, OptionalExtension};

use super::Inner;
use crate::api::Envelope;
use crate::error::Result;
use crate::util::now_ms;

/// Resend requests are answered at most this many times per message...
const MAX_RESENDS: i64 = 5;
/// ...and only for messages up to this age.
const RESEND_MAX_AGE_MS: i64 = 2 * 24 * 3600 * 1000;

impl Inner {
    /// Shows "a message could not be decrypted" in the conversation with the sender (at most
    /// one notice per sender and hour; never creates a conversation). It is removed again when
    /// the sender's resend arrives (see `clear_undecryptable`).
    pub(super) fn note_undecryptable(&self, env: &Envelope) -> Result<()> {
        let conv = match (&env.group_id, env.kind) {
            (Some(g), 7) => self.conversation_id_for_group(g)?,
            _ => self.conversation_id_for_peer(&env.source_account)?,
        };
        let Some(conv) = conv else { return Ok(()) };
        let hour = now_ms() / 3_600_000;
        self.insert_system_message(
            &conv,
            "decryption_failed",
            Some(env.source_account.clone()),
            vec![],
            Some(env.timestamp.to_string()),
            Some(format!("undecryptable-{}-{hour}", env.source_account)),
            env.timestamp.min(now_ms()),
        )
    }

    /// The message sent by `sender` at `sent_at` arrived after all: drop its placeholder.
    pub(super) fn clear_undecryptable(&self, sender: &str, sent_at: i64) -> Result<()> {
        let id: Option<String> = self
            .db
            .borrow()
            .conn
            .query_row(
                "SELECT id FROM messages WHERE kind = 'system' AND id LIKE 'sys-undecryptable-' || ?1 || '-%'
                   AND json_extract(system_json, '$.value') = ?2 LIMIT 1",
                params![sender, sent_at.to_string()],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(id) = id {
            self.delete_message_local(&id)?;
        }
        Ok(())
    }

    /// Whether we may answer another resend request for our message `message_id` sent at
    /// `sent_at` (and counts this one).
    pub(super) fn allow_resend(&self, message_id: &str, sent_at: i64) -> Result<bool> {
        if now_ms() - sent_at > RESEND_MAX_AGE_MS {
            return Ok(false);
        }
        let key = format!("resent:{message_id}");
        let db = self.db.borrow();
        let n = db.kv_get_i64(&key)?.unwrap_or(0);
        if n >= MAX_RESENDS {
            return Ok(false);
        }
        db.kv_set_json(&key, &(n + 1))?;
        Ok(true)
    }
}
