//! View-once media: a photo, video or voice message the recipient can open a single time.
//!
//! The sender marks the message `view_once` (exactly one image, video or voice note, no
//! text). On the receiving side the media is downloaded as usual but never handed to the UI
//! inline: the message model carries no path and no thumbnail, only a placeholder state. `open_view_once`
//! marks the message opened (here, on our other devices through a sync message, and for the
//! sender through a `Viewed` receipt when read receipts are on) and returns the attachment so
//! the viewer can show it; `view_once_done` (when the viewer closes) overwrites and deletes the
//! local file and forgets the attachment key. Anything opened but not finished (the app was
//! closed while viewing) is erased at the next start. Unopened media expires after 30 days.
//!
//! The sender keeps no viewable copy either: its local file goes once the message is sent.
//!
//! Voice notes are played once the same way: the apps play the decrypted bytes from memory
//! (desktop: a blob URL) or a private temporary copy deleted when playback ends (mobile), and
//! call `view_once_done` when playback finishes, the player closes or the app goes to the
//! background, whichever comes first. No replay: the file is gone after that.
//!
//! View-once messages are never forwarded, saved, listed in the media gallery or backed up
//! to the vault. A modified client can of course keep a copy: this is about honest clients
//! not leaving the media around, not DRM.

use std::io::{Seek, Write};
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

use rusqlite::params;

use super::send::Job;
use super::Inner;
use crate::content::{Content, ReceiptKind, SyncMessage};
use crate::error::{CoreError, Result};
use crate::model::{Attachment, StoredAttachment};
use crate::util::now_ms;

pub const UNOPENED: i64 = 1;
pub const OPENED: i64 = 2;
pub const EXPIRED: i64 = 3;

/// Unopened view-once media is erased after this long.
pub const VIEW_ONCE_MAX_AGE_MS: i64 = 30 * 24 * 3600 * 1000;

/// The UI label of a view-once state (None: an ordinary message).
pub fn state_label(state: i64) -> Option<String> {
    match state {
        0 => None,
        UNOPENED => Some("unopened".into()),
        OPENED => Some("opened".into()),
        _ => Some("expired".into()),
    }
}

/// Whether an attachment kind can be sent view-once: a photo, a video or a voice note.
pub fn valid_view_once_kind(kind: &str) -> bool {
    kind == "image" || kind == "video" || kind == "voice"
}

/// Whether a message has the shape a view-once message must have: exactly one photo, video or
/// voice attachment and nothing else (`has_other_content`: text, location or contacts).
pub fn valid_view_once_shape<'a>(has_other_content: bool, kinds: impl IntoIterator<Item = &'a str>) -> bool {
    let kinds: Vec<&str> = kinds.into_iter().collect();
    !has_other_content && kinds.len() == 1 && valid_view_once_kind(kinds[0])
}

/// Overwrites a file with zeros (flushed to disk) before removing it. Best effort: SSDs and
/// copy-on-write file systems may keep old blocks, but the file is also encrypted at rest with
/// the media key (engine/media.rs), so what could remain is ciphertext.
pub fn secure_remove(path: &Path) {
    let overwrite = || -> std::io::Result<()> {
        let mut f = std::fs::OpenOptions::new().write(true).open(path)?;
        let len = f.metadata()?.len();
        f.seek(std::io::SeekFrom::Start(0))?;
        let zeros = vec![0u8; 64 * 1024];
        let mut left = len;
        while left > 0 {
            let n = left.min(zeros.len() as u64) as usize;
            f.write_all(&zeros[..n])?;
            left -= n as u64;
        }
        f.sync_all()
    };
    if let Err(e) = overwrite() {
        if e.kind() != std::io::ErrorKind::NotFound {
            log::warn!("view-once: cannot overwrite media before deleting it: {e}");
        }
    }
    let _ = std::fs::remove_file(path);
}

impl Inner {
    fn set_view_once_state(&self, id: &str, state: i64) -> Result<()> {
        self.db.borrow().conn.execute("UPDATE messages SET view_once = ?2 WHERE id = ?1", params![id, state])?;
        Ok(())
    }

    /// Marks a just-inserted message as view-once (called by the send and receive paths).
    pub(super) fn mark_view_once(&self, id: &str) -> Result<()> {
        self.set_view_once_state(id, UNOPENED)
    }

    /// Deletes the local copy of a view-once message's media. With `forget_key`, the
    /// attachment key and server pointer go too (nothing can download it again); a sent
    /// message keeps them so a recipient's resend request can still be answered.
    pub(super) fn erase_view_once_media(&self, id: &str, forget_key: bool) -> Result<()> {
        let Some(row) = self.message_row(id)? else { return Ok(()) };
        if row.view_once == 0 {
            return Ok(());
        }
        let mut changed = false;
        let mut atts: Vec<StoredAttachment> = row.attachments.clone();
        for a in atts.iter_mut() {
            if let Some(p) = a.local_path.take() {
                let path = Path::new(&p);
                if self.is_own_media(path) && !self.media_shared(&p, id) {
                    secure_remove(path);
                }
                changed = true;
            }
            if a.thumbnail.take().is_some() {
                changed = true;
            }
            if forget_key && (a.key.is_some() || a.cdn_id.is_some() || a.digest.is_some()) {
                a.key = None;
                a.cdn_id = None;
                a.digest = None;
                a.download_state = "done".into();
                changed = true;
            }
        }
        if changed {
            self.update_attachments(id, &atts)?;
        }
        Ok(())
    }

    /// Opens a received view-once message: downloads it if needed, marks it opened everywhere
    /// and returns the attachment (with its local path) for the viewer. Call `view_once_done`
    /// when the viewer closes.
    pub async fn open_view_once(self: &Rc<Self>, id: &str) -> Result<Attachment> {
        let row = self.message_row(id)?.ok_or_else(|| CoreError::not_found("message not found"))?;
        if row.view_once == 0 || row.outgoing || row.deleted {
            return Err(CoreError::invalid("not a received view-once message"));
        }
        if row.view_once != UNOPENED {
            return Err(CoreError::invalid("this view-once media was already opened"));
        }
        let att = row.attachments.first().cloned().ok_or_else(|| CoreError::not_found("attachment not found"))?;
        let downloaded = att.download_state == "done" && att.local_path.as_ref().is_some_and(|p| Path::new(p).exists());
        if !downloaded {
            self.download_attachment(id, &att.id).await?;
        }
        // Re-read: the download fills in the path (and may have changed state meanwhile).
        let row = self.message_row(id)?.ok_or_else(|| CoreError::not_found("message not found"))?;
        if row.view_once != UNOPENED {
            return Err(CoreError::invalid("this view-once media was already opened"));
        }
        let att = row.attachments.first().cloned().ok_or_else(|| CoreError::not_found("attachment not found"))?;
        if att.local_path.is_none() {
            return Err(CoreError::internal("view-once media is not available"));
        }
        if !valid_view_once_kind(&att.kind) {
            // Not a real photo, video or voice note after all (engine/attachments.rs checks
            // image bytes and turns what does not decode into a plain file).
            self.set_view_once_state(id, EXPIRED)?;
            self.erase_view_once_media(id, true)?;
            self.emit_message(id, false);
            return Err(CoreError::invalid("this view-once media cannot be shown"));
        }
        self.set_view_once_state(id, OPENED)?;
        self.db.borrow().conn.execute("UPDATE messages SET read_local = 1 WHERE id = ?1", [id])?;
        self.recount_unread(&row.conversation_id)?;
        // The sender learns it was opened, like a read receipt (same privacy setting, never to
        // message requests or blocked accounts).
        let sender = row.sender_account_id.clone();
        if self.privacy()?.read_receipts && self.conversation_accepted_id(&row.conversation_id) && !self.is_blocked(&sender) {
            self.enqueue(&Job::Account {
                account_id: sender.clone(),
                content: Content::Receipt { kind: ReceiptKind::Viewed, ids: vec![id.to_string()] },
            })?;
        }
        self.enqueue(&Job::OwnDevices {
            content: Content::Sync(SyncMessage::ViewOnceOpened { sender, id: id.to_string() }),
        })?;
        self.emit_message(id, false);
        self.emit_conversation(&row.conversation_id);
        Ok(Attachment { thumbnail: None, ..Attachment::from(&att) })
    }

    /// The viewer closed: the media is erased for good.
    pub fn view_once_done(&self, id: &str) -> Result<()> {
        let Some(row) = self.message_row(id)? else { return Ok(()) };
        if row.view_once == 0 || row.outgoing {
            return Err(CoreError::invalid("not a received view-once message"));
        }
        if row.view_once == UNOPENED {
            // Never opened (the viewer failed before `open_view_once`): keep it.
            return Ok(());
        }
        self.erase_view_once_media(id, true)?;
        self.emit_message(id, false);
        Ok(())
    }

    /// Another of our devices opened a view-once message we received.
    pub(super) fn apply_view_once_opened_sync(&self, sender: &str, id: &str) -> Result<()> {
        let Some(row) = self.message_row(id)? else { return Ok(()) };
        if row.view_once == 0 || row.outgoing || row.sender_account_id != sender {
            return Ok(());
        }
        if row.view_once == UNOPENED {
            self.set_view_once_state(id, OPENED)?;
            self.db.borrow().conn.execute("UPDATE messages SET read_local = 1 WHERE id = ?1", [id])?;
            self.recount_unread(&row.conversation_id)?;
        }
        self.erase_view_once_media(id, true)?;
        self.emit_message(id, false);
        self.emit_conversation(&row.conversation_id);
        Ok(())
    }

    /// A recipient opened our view-once message.
    pub(super) fn on_view_once_viewed(&self, id: &str) -> Result<()> {
        let Some(row) = self.message_row(id)? else { return Ok(()) };
        if row.view_once == UNOPENED && row.outgoing {
            self.set_view_once_state(id, OPENED)?;
            self.emit_message(id, false);
        }
        Ok(())
    }

    /// Our view-once message went out: we keep no viewable copy.
    pub(super) fn after_view_once_sent(&self, id: &str) -> Result<()> {
        let Some(row) = self.message_row(id)? else { return Ok(()) };
        if row.view_once != 0 && row.outgoing {
            self.erase_view_once_media(id, false)?;
        }
        Ok(())
    }

    /// Expires unopened media older than 30 days and erases media that was opened but not
    /// finished (the app closed while viewing; only at start, so a viewer that is open now is
    /// not cut short) or that a sent message still holds.
    pub fn view_once_housekeeping(&self, at_start: bool) -> Result<()> {
        let cutoff = now_ms() - VIEW_ONCE_MAX_AGE_MS;
        let expired: Vec<String> = {
            let db = self.db.borrow();
            let mut st = db.conn.prepare(
                "SELECT id FROM messages WHERE view_once = ?1 AND outgoing = 0 AND received_at < ?2",
            )?;
            let rows = st.query_map(params![UNOPENED, cutoff], |r| r.get(0))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        for id in expired {
            self.set_view_once_state(&id, EXPIRED)?;
            self.erase_view_once_media(&id, true)?;
            self.emit_message(&id, false);
        }
        if at_start {
            let leftovers: Vec<(String, bool, String)> = {
                let db = self.db.borrow();
                let mut st = db.conn.prepare(
                    "SELECT id, outgoing, status FROM messages WHERE view_once != 0 AND attachments_json LIKE '%\"local_path\":\"%'",
                )?;
                let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
                rows.collect::<rusqlite::Result<_>>()?
            };
            for (id, outgoing, status) in leftovers {
                let state: i64 = self.message_row(&id)?.map(|r| r.view_once).unwrap_or(0);
                if outgoing {
                    // Unsent ones still need their file to upload.
                    if status != "pending" && status != "failed" {
                        self.erase_view_once_media(&id, false)?;
                    }
                } else if state != UNOPENED {
                    self.erase_view_once_media(&id, true)?;
                }
            }
        }
        Ok(())
    }

    pub(super) async fn view_once_loop(self: Rc<Self>) {
        let mut at_start = true;
        loop {
            if let Err(e) = self.view_once_housekeeping(at_start) {
                log::warn!("view-once housekeeping: {e}");
            }
            at_start = false;
            tokio::time::sleep(Duration::from_secs(3600)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels() {
        assert_eq!(state_label(0), None);
        assert_eq!(state_label(UNOPENED).as_deref(), Some("unopened"));
        assert_eq!(state_label(OPENED).as_deref(), Some("opened"));
        assert_eq!(state_label(EXPIRED).as_deref(), Some("expired"));
        assert!(valid_view_once_kind("image") && valid_view_once_kind("video") && valid_view_once_kind("voice"));
        assert!(!valid_view_once_kind("file") && !valid_view_once_kind("location") && !valid_view_once_kind(""));
    }

    #[test]
    fn shape() {
        // A single photo, video or voice note, alone.
        assert!(valid_view_once_shape(false, ["voice"]));
        assert!(valid_view_once_shape(false, ["image"]));
        assert!(valid_view_once_shape(false, ["video"]));
        // Voice with text (or a location / contacts), two attachments, nothing, a file: refused.
        assert!(!valid_view_once_shape(true, ["voice"]));
        assert!(!valid_view_once_shape(false, ["voice", "voice"]));
        assert!(!valid_view_once_shape(false, ["voice", "image"]));
        assert!(!valid_view_once_shape(false, []));
        assert!(!valid_view_once_shape(false, ["file"]));
    }

    #[test]
    fn secure_remove_deletes() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.bin");
        std::fs::write(&p, vec![7u8; 200_000]).unwrap();
        secure_remove(&p);
        assert!(!p.exists());
        secure_remove(&p); // missing file: no panic
    }
}
