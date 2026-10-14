//! Edit history: the earlier versions of an edited message are kept with it (column
//! `messages.edit_history_json`), so an edit cannot silently rewrite what was said. A message
//! takes at most `MAX_EDITS` edits; further ones are refused (ours) or ignored (received).

use rusqlite::{params, OptionalExtension};

use super::Inner;
use crate::error::Result;
use crate::model::EditVersion;

pub const MAX_EDITS: usize = 10;

impl Inner {
    fn stored_edit_history(&self, id: &str) -> Result<Vec<EditVersion>> {
        let raw: Option<Option<String>> = self
            .db
            .borrow()
            .conn
            .query_row("SELECT edit_history_json FROM messages WHERE id = ?1", [id], |r| r.get(0))
            .optional()?;
        Ok(raw.flatten().and_then(|j| serde_json::from_str(&j).ok()).unwrap_or_default())
    }

    /// Number of edits a message already had.
    pub fn edit_count(&self, id: &str) -> Result<usize> {
        Ok(self.stored_edit_history(id)?.len())
    }

    /// Replaces the text of a message, keeping the current one in its history. Returns false
    /// (nothing changed) for a deleted message or one that reached `MAX_EDITS`.
    pub(super) fn apply_edit(&self, id: &str, text: &str, mentions_json: Option<String>, at: i64) -> Result<bool> {
        let current: Option<(Option<String>, i64, Option<i64>, bool)> = self
            .db
            .borrow()
            .conn
            .query_row("SELECT body, sent_at, edited_at, deleted FROM messages WHERE id = ?1", [id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })
            .optional()?;
        let Some((body, sent_at, edited_at, deleted)) = current else { return Ok(false) };
        if deleted {
            return Ok(false);
        }
        if body.as_deref() == Some(text) {
            return Ok(true); // the same edit again (duplicate delivery, our own echo)
        }
        let mut history = self.stored_edit_history(id)?;
        if history.len() >= MAX_EDITS {
            return Ok(false);
        }
        history.push(EditVersion { text: body.unwrap_or_default(), at: edited_at.unwrap_or(sent_at) });
        self.db.borrow().conn.execute(
            "UPDATE messages SET body = ?2, edited_at = ?3, mentions_json = ?4, edit_history_json = ?5 WHERE id = ?1 AND deleted = 0",
            params![id, text, at, mentions_json, serde_json::to_string(&history)?],
        )?;
        Ok(true)
    }

    /// Every version of a message, oldest first; the last one is the current text. Empty for
    /// a message that was never edited (or was deleted).
    pub fn get_edit_history(&self, id: &str) -> Result<Vec<EditVersion>> {
        let Some(row) = self.message_row(id)? else { return Err(crate::error::CoreError::not_found("message not found")) };
        if row.deleted || row.edited_at.is_none() {
            return Ok(vec![]);
        }
        let mut versions = self.stored_edit_history(id)?;
        versions.push(EditVersion { text: row.body.unwrap_or_default(), at: row.edited_at.unwrap_or(row.sent_at) });
        Ok(versions)
    }
}

#[cfg(test)]
mod tests {
    use crate::model::EditVersion;

    #[test]
    fn history_json_roundtrip() {
        let v = vec![EditVersion { text: "a".into(), at: 1 }, EditVersion { text: "b".into(), at: 2 }];
        let j = serde_json::to_string(&v).unwrap();
        assert_eq!(serde_json::from_str::<Vec<EditVersion>>(&j).unwrap(), v);
    }
}
