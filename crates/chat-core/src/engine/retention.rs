//! Media retention: photos, videos, voice messages and files are erased from this device after
//! the retention period, whether the conversation still exists or not, read or not, sent or
//! received. The period is chosen per account (1, 7, 14 or 30 days; 30 is the default and the
//! longest allowed: there is no "keep forever") and reaches our other devices through the vault
//! settings record (newest change wins, like engine/dnd.rs).
//!
//! Age reference: the earlier of `sent_at` (the sender's clock) and `received_at` (ours). A
//! message delivered late (offline device, resend) does not get a fresh period, and a sender
//! that puts its clock in the future cannot extend it either (`received_at` caps it).
//!
//! Erasing a message's media (`apply_media_retention`, at start, then when the oldest media is
//! due and at least hourly from the expiry loop, and at once when the period is changed):
//! - the local files are overwritten and deleted. A forwarded copy that shares the file loses
//!   it too (the content is that old), and is erased the same way,
//! - each attachment in `attachments_json` is rewritten to an "expired" stub: kind, type, size
//!   and, for documents only, the file name. The decryption key, digest, server id, preview
//!   thumbnail, waveform, dimensions and local path are dropped, so the media cannot be
//!   downloaded again (the server deletes its copy after 30 days anyway),
//! - the vault record is rewritten (restores and other devices get the stub, not the key) and
//!   the write-ahead log is checkpointed so the old JSON does not linger on disk.
//!
//! The message row stays: a voice note or a photo without caption is shown as a placeholder.
//! Messages that arrive (or are restored) already past the period are stored expired, and
//! downloads, saves and forwards of expired media fail with `Gone` ("expired").
//!
//! The media check (`run_media_check`, after every retention pass, so at least hourly, and on
//! demand) makes sure no file survives outside these rules: in the media folders it removes
//! files no live attachment or avatar uses (older than two minutes: a download or a send may be
//! writing them), and any message file older than the period, whatever references it (those
//! messages are erased as above). The apps sweep their own temporary copies (mobile: Dart side).
//!
//! View-once media has its own lifecycle (engine/view_once.rs) and is left to it. Avatars are
//! not message media and are not affected.

use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::SystemTime;

use rusqlite::params;
use serde::{Deserialize, Serialize};

use super::messages::MessageRow;
use super::Inner;
use crate::error::{Context, CoreError, ErrorCode, Result};
use crate::model::StoredAttachment;
use crate::util::now_ms;

/// The longest (and default) retention period.
pub const MEDIA_RETENTION_DAYS: i64 = 30;
/// The periods the user can choose from.
pub const MEDIA_RETENTION_OPTIONS: [i64; 4] = [1, 7, 14, 30];
/// The pass runs at least this often (and also when the oldest media becomes due).
const MAX_PASS_INTERVAL_MS: i64 = 3600 * 1000;
/// `download_state` of erased media.
pub const EXPIRED_STATE: &str = "expired";
const KV_SETTING: &str = "media_retention";
const KV_CHECK: &str = "media_check";

/// Length of a "day" for the retention in ms (only tests shorten it).
static DAY_MS: AtomicI64 = AtomicI64::new(24 * 3600 * 1000);

fn day_ms() -> i64 {
    DAY_MS.load(Ordering::Relaxed)
}

/// TEST ONLY: shortens the retention clock for the whole process (all cores in it) so that the
/// longest period (30 days) lasts `max_period`; shorter periods scale with it.
#[cfg(feature = "insecure-test-hooks")]
pub fn set_media_retention_for_tests(max_period: std::time::Duration) {
    DAY_MS.store((max_period.as_millis() as i64 / MEDIA_RETENTION_DAYS).max(1), Ordering::Relaxed);
}

/// When a message's media starts aging: the earlier of the sender's and our timestamp.
pub fn media_age_reference(sent_at: i64, received_at: i64) -> i64 {
    sent_at.min(received_at)
}

/// Whether media of a message sent/received at these times is older than `limit_ms`.
pub fn media_past_retention(sent_at: i64, received_at: i64, now: i64, limit_ms: i64) -> bool {
    now.saturating_sub(media_age_reference(sent_at, received_at)) >= limit_ms
}

/// Whether an attachment was erased by the retention.
pub fn is_expired(a: &StoredAttachment) -> bool {
    a.expired_at.is_some() || a.download_state == EXPIRED_STATE
}

/// The error for any use of expired media.
pub fn expired_error() -> CoreError {
    CoreError::new(ErrorCode::Gone, "expired")
}

/// The stub left of an attachment once its media is erased: kind, type, size and (documents
/// only) the file name. No key, digest, server id, path, preview or waveform.
pub fn expired_attachment(a: &StoredAttachment, now: i64) -> StoredAttachment {
    let keep_name = !matches!(a.kind.as_str(), "image" | "video" | "voice");
    StoredAttachment {
        id: a.id.clone(),
        mime: a.mime.clone(),
        file_name: if keep_name { a.file_name.clone() } else { String::new() },
        size: a.size,
        kind: a.kind.clone(),
        download_state: EXPIRED_STATE.into(),
        expired_at: Some(a.expired_at.unwrap_or(now)),
        ..Default::default()
    }
}

/// A valid choice of period (anything else is refused, never rounded up).
pub fn valid_retention_days(days: i64) -> bool {
    MEDIA_RETENTION_OPTIONS.contains(&days)
}

/// Stored setting (also the vault copy).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MediaRetentionRecord {
    #[serde(default = "default_days")]
    pub days: i64,
    /// When it was changed (ms), to keep the newest change across devices; 0 = never set.
    #[serde(default)]
    pub updated_at: i64,
}

fn default_days() -> i64 {
    MEDIA_RETENTION_DAYS
}

impl Default for MediaRetentionRecord {
    fn default() -> Self {
        MediaRetentionRecord { days: MEDIA_RETENTION_DAYS, updated_at: 0 }
    }
}

/// Result of a media check (`run_media_check`; also the last one, kept in the database).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct MediaCheck {
    /// Files removed by that check.
    pub removed: i64,
    pub checked_at: i64,
}

/// Payload of `get_media_retention` / `set_media_retention` and of the
/// `media_retention_changed` event.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MediaRetention {
    pub days: i64,
    pub options: Vec<i64>,
    pub max_days: i64,
    /// The last media check (None: none yet).
    pub last_check: Option<MediaCheck>,
}

/// Rows whose media the retention still has to erase.
const PENDING_FILTER: &str = "deleted = 0 AND view_once = 0 AND attachments_json IS NOT NULL AND attachments_json != '[]'
    AND instr(attachments_json, '\"expired_at\"') = 0";

/// A message whose media is to be erased.
struct Due {
    table: &'static str,
    id: String,
    conv: String,
    atts: Vec<StoredAttachment>,
}

/// How a path appears inside the attachments JSON.
fn json_needle(path: &Path) -> String {
    let s = serde_json::to_string(&path.to_string_lossy()).unwrap_or_default();
    s.trim_matches('"').to_string()
}

impl Inner {
    pub(super) fn media_retention_record(&self) -> MediaRetentionRecord {
        self.db.borrow().kv_get_json(KV_SETTING).ok().flatten().unwrap_or_default()
    }

    /// The period in days (a damaged value counts as the longest allowed, never longer).
    pub fn media_retention_days(&self) -> i64 {
        let d = self.media_retention_record().days;
        if valid_retention_days(d) {
            d
        } else {
            MEDIA_RETENTION_DAYS
        }
    }

    /// The period in ms.
    pub fn media_retention_limit_ms(&self) -> i64 {
        self.media_retention_days() * day_ms()
    }

    /// Whether media of a message sent/received at these times is past the period now.
    pub(super) fn media_past_retention(&self, sent_at: i64, received_at: i64) -> bool {
        media_past_retention(sent_at, received_at, now_ms(), self.media_retention_limit_ms())
    }

    /// Whether a message's media can no longer be used: erased, or past the period (and about
    /// to be erased).
    pub(super) fn message_media_expired(&self, row: &MessageRow) -> bool {
        !row.attachments.is_empty()
            && (row.attachments.iter().any(is_expired) || self.media_past_retention(row.sent_at, row.received_at))
    }

    fn last_media_check(&self) -> Option<MediaCheck> {
        self.db.borrow().kv_get_json(KV_CHECK).ok().flatten()
    }

    pub fn get_media_retention(&self) -> MediaRetention {
        MediaRetention {
            days: self.media_retention_days(),
            options: MEDIA_RETENTION_OPTIONS.to_vec(),
            max_days: MEDIA_RETENTION_DAYS,
            last_check: self.last_media_check(),
        }
    }

    /// Changes the period (one of `MEDIA_RETENTION_OPTIONS`) on all our devices; media older
    /// than the new period is erased at once.
    pub async fn set_media_retention(&self, days: i64) -> Result<MediaRetention> {
        if !valid_retention_days(days) {
            return Err(CoreError::invalid("media retention must be 1, 7, 14 or 30 days"));
        }
        let cur = self.media_retention_record();
        let r = MediaRetentionRecord { days, updated_at: now_ms().max(cur.updated_at + 1) };
        self.db.borrow().kv_set_json(KV_SETTING, &r)?;
        self.vault_put_settings().await;
        self.apply_media_retention().await?;
        self.run_media_check().await?;
        // The expiry loop reschedules for the new period.
        self.expiry_notify.notify_one();
        let state = self.get_media_retention();
        self.emit("media_retention_changed", &state);
        Ok(state)
    }

    /// The setting from another of our devices (vault settings record): newest change wins.
    pub(super) fn apply_synced_media_retention(&self, remote: MediaRetentionRecord) -> Result<()> {
        if remote.updated_at <= self.media_retention_record().updated_at || !valid_retention_days(remote.days) {
            return Ok(());
        }
        let before = self.media_retention_days();
        self.db.borrow().kv_set_json(KV_SETTING, &remote)?;
        if self.media_retention_days() != before {
            // The expiry loop sees the new period and runs the pass.
            self.expiry_notify.notify_one();
            self.emit("media_retention_changed", &self.get_media_retention());
        }
        Ok(())
    }

    /// Not yet erased messages matching `filter` (SQL on the messages table, one parameter),
    /// in the main and the temporary (engine/temporary.rs) tables.
    fn due_rows(&self, filter: &str, param: &dyn rusqlite::ToSql) -> Result<Vec<Due>> {
        let db = self.db.borrow();
        let mut out = Vec::new();
        for table in ["main.messages", "temp.messages"] {
            let Ok(mut st) =
                db.conn.prepare(&format!("SELECT id, conversation_id, attachments_json FROM {table} WHERE {PENDING_FILTER} AND {filter}"))
            else {
                continue; // no temp table
            };
            let rows = st.query_map([param], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))?;
            for row in rows {
                let (id, conv, json) = row?;
                out.push(Due { table, id, conv, atts: serde_json::from_str(&json).unwrap_or_default() });
            }
        }
        Ok(out)
    }

    /// Erases the media of these messages and of any other message sharing their files.
    /// Returns (messages changed, files removed).
    async fn erase_media(&self, mut due: Vec<Due>) -> Result<(usize, usize)> {
        if due.is_empty() {
            return Ok((0, 0));
        }
        let now = now_ms();
        let mut seen: HashSet<(&'static str, String)> = due.iter().map(|d| (d.table, d.id.clone())).collect();
        let mut files: BTreeSet<PathBuf> = BTreeSet::new();
        let mut i = 0;
        while i < due.len() {
            let paths: Vec<PathBuf> = due[i].atts.iter().filter_map(|a| a.local_path.as_deref()).map(PathBuf::from).collect();
            for path in paths {
                if !self.is_own_media(&path) || !files.insert(path.clone()) {
                    continue;
                }
                // Forwarded copies share the file: they go with it.
                for d in self.due_rows("instr(attachments_json, ?1) > 0", &json_needle(&path))? {
                    if seen.insert((d.table, d.id.clone())) {
                        due.push(d);
                    }
                }
            }
            i += 1;
        }
        for d in &due {
            let expired: Vec<StoredAttachment> = d.atts.iter().map(|a| expired_attachment(a, now)).collect();
            self.db.borrow().conn.execute(
                &format!("UPDATE {} SET attachments_json = ?2 WHERE id = ?1", d.table),
                params![d.id, serde_json::to_string(&expired)?],
            )?;
        }
        let files: Vec<PathBuf> = files.into_iter().filter(|f| f.exists()).collect();
        let n_files = files.len();
        tokio::task::spawn_blocking(move || {
            for f in files {
                super::view_once::secure_remove(&f);
            }
        })
        .await
        .ctx("media retention task")?;
        // The old attachments JSON (keys, thumbnails) may still sit in the write-ahead log.
        if let Err(e) = self.db.borrow().conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);") {
            log::warn!("wal checkpoint after media retention: {e}");
        }
        let mut convs = BTreeSet::new();
        for d in &due {
            if d.table == "main.messages" {
                self.vault_put_message(&d.id);
            }
            self.emit_message(&d.id, false);
            convs.insert(d.conv.clone());
        }
        for conv in convs {
            self.emit_conversation(&conv);
        }
        log::info!("media retention: erased the media of {} messages ({n_files} files)", due.len());
        Ok((due.len(), n_files))
    }

    /// Erases the media of every message past the period (see the module docs). Returns the
    /// number of messages changed.
    pub async fn apply_media_retention(&self) -> Result<usize> {
        let cutoff = now_ms() - self.media_retention_limit_ms();
        let due = self.due_rows("MIN(sent_at, received_at) <= ?1", &cutoff)?;
        Ok(self.erase_media(due).await?.0)
    }

    /// Makes sure no media file survives outside the rules (see the module docs) and records
    /// when it ran and how many files it removed.
    pub async fn run_media_check(&self) -> Result<MediaCheck> {
        let limit = std::time::Duration::from_millis(self.media_retention_limit_ms().max(0) as u64);
        let messages = self.referenced_message_media()?;
        let avatars = self.referenced_avatars()?;
        let now = SystemTime::now();
        let mut stray = Vec::new();
        let mut too_old = Vec::new();
        for dir in [self.media_dir(), self.temp_media_dir()] {
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            for e in entries.flatten() {
                let p = e.path();
                if !p.is_file() || avatars.contains(&p) {
                    continue;
                }
                let age = e.metadata().and_then(|m| m.modified()).map(|t| now.duration_since(t).unwrap_or_default()).unwrap_or_default();
                if messages.contains(&p) {
                    if age >= limit {
                        too_old.push(p);
                    }
                } else if age >= super::media::SWEEP_MIN_AGE {
                    stray.push(p);
                }
            }
        }
        let mut removed = stray.len();
        tokio::task::spawn_blocking(move || {
            for f in stray {
                super::view_once::secure_remove(&f);
            }
        })
        .await
        .ctx("media check task")?;
        // A message file older than the period: whatever still uses it is erased with it.
        let mut due = Vec::new();
        let mut seen = HashSet::new();
        for p in &too_old {
            for d in self.due_rows("instr(attachments_json, ?1) > 0", &json_needle(p))? {
                if seen.insert((d.table, d.id.clone())) {
                    due.push(d);
                }
            }
        }
        removed += self.erase_media(due).await?.1;
        let check = MediaCheck { removed: removed as i64, checked_at: now_ms() };
        self.db.borrow().kv_set_json(KV_CHECK, &check)?;
        if removed > 0 {
            log::info!("media check: removed {removed} files");
        }
        self.emit("media_check", &check);
        Ok(check)
    }

    /// When the next media becomes due (None: no media waiting).
    fn next_media_retention_due(&self) -> Option<i64> {
        let db = self.db.borrow();
        ["main.messages", "temp.messages"]
            .iter()
            .filter_map(|table| {
                db.conn
                    .query_row(&format!("SELECT MIN(MIN(sent_at, received_at)) FROM {table} WHERE {PENDING_FILTER}"), [], |r| {
                        r.get::<_, Option<i64>>(0)
                    })
                    .ok()
                    .flatten()
            })
            .min()
            .map(|t| t.saturating_add(self.media_retention_limit_ms()))
    }

    /// Runs the retention pass and the media check; returns when they are due next (called by
    /// the expiry loop).
    pub(super) async fn media_retention_tick(&self) -> i64 {
        if let Err(e) = self.apply_media_retention().await {
            log::warn!("media retention: {e}");
        }
        if let Err(e) = self.run_media_check().await {
            log::warn!("media check: {e}");
        }
        let now = now_ms();
        let latest = (now + MAX_PASS_INTERVAL_MS.min(self.media_retention_limit_ms())).max(now + 1000);
        // Not sooner than a second from now: a row that cannot be updated must not spin.
        self.next_media_retention_due().unwrap_or(i64::MAX).clamp(now + 1000, latest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full() -> StoredAttachment {
        StoredAttachment {
            id: "m1-0".into(),
            mime: "image/jpeg".into(),
            file_name: "IMG_0042.jpg".into(),
            size: 12345,
            width: Some(640),
            height: Some(480),
            duration_ms: Some(1000),
            waveform: Some(vec![0.1, 0.5]),
            thumbnail: Some("data:image/jpeg;base64,AAAA".into()),
            local_path: Some("/data/media/m1-0.jpg".into()),
            download_state: "done".into(),
            cdn_id: Some("cdn-123".into()),
            key: Some("a2V5".into()),
            digest: Some("ZGlnZXN0".into()),
            kind: "image".into(),
            expired_at: None,
        }
    }

    #[test]
    fn rewrite_drops_everything_sensitive() {
        let e = expired_attachment(&full(), 42);
        assert_eq!(e.id, "m1-0");
        assert_eq!((e.kind.as_str(), e.mime.as_str(), e.size), ("image", "image/jpeg", 12345));
        assert_eq!(e.download_state, EXPIRED_STATE);
        assert_eq!(e.expired_at, Some(42));
        assert!(e.key.is_none() && e.digest.is_none() && e.cdn_id.is_none());
        assert!(e.thumbnail.is_none() && e.local_path.is_none() && e.waveform.is_none());
        assert!(e.width.is_none() && e.height.is_none() && e.duration_ms.is_none());
        assert!(e.file_name.is_empty(), "photo names are dropped");
        assert!(is_expired(&e) && !is_expired(&full()));
        // The JSON stored in the database has none of it either.
        let json = serde_json::to_string(&e).unwrap();
        for secret in ["cdn-123", "a2V5", "ZGlnZXN0", "base64", "m1-0.jpg", "IMG_0042"] {
            assert!(!json.contains(secret), "{secret} left in {json}");
        }
        // Expiring again keeps the first time.
        assert_eq!(expired_attachment(&e, 99).expired_at, Some(42));
    }

    #[test]
    fn document_names_are_kept() {
        let doc = StoredAttachment { kind: "file".into(), file_name: "contract.pdf".into(), ..full() };
        assert_eq!(expired_attachment(&doc, 1).file_name, "contract.pdf");
        let voice = StoredAttachment { kind: "voice".into(), file_name: "voice-1.webm".into(), ..full() };
        assert!(expired_attachment(&voice, 1).file_name.is_empty());
    }

    #[test]
    fn age_counts_from_the_earlier_time() {
        let day = 24 * 3600 * 1000;
        let (now, limit) = (100 * day, 30 * day);
        // Delivered late: counts from sending.
        assert!(media_past_retention(now - 31 * day, now - day, now, limit));
        // Sender clock in the future: counts from receiving.
        assert!(!media_past_retention(now + 365 * day, now - day, now, limit));
        assert!(media_past_retention(now + 365 * day, now - 30 * day, now, limit));
        assert!(!media_past_retention(now - 29 * day, now - 29 * day, now, limit));
        assert!(media_past_retention(now - 2 * day, now - 2 * day, now, day));
    }

    #[test]
    fn only_the_listed_periods_are_valid() {
        for d in [1, 7, 14, 30] {
            assert!(valid_retention_days(d));
        }
        for d in [0, -1, 2, 15, 29, 31, 60, 365, i64::MAX] {
            assert!(!valid_retention_days(d), "{d}");
        }
        // Old or partial vault records fall back to the longest allowed period.
        let r: MediaRetentionRecord = serde_json::from_str(r#"{"updated_at":5}"#).unwrap();
        assert_eq!(r, MediaRetentionRecord { days: 30, updated_at: 5 });
    }

    #[tokio::test]
    async fn set_validate_and_sync() {
        use crate::engine::{Core, CoreConfig, EventSink};
        use std::sync::{Arc, Mutex};
        #[derive(Default)]
        struct Events(Mutex<Vec<String>>);
        impl EventSink for Events {
            fn emit(&self, e: &str, _: serde_json::Value) {
                self.0.lock().unwrap().push(e.to_string());
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let events = Arc::new(Events::default());
        let core = Core::start(CoreConfig {
            data_dir: dir.path().to_path_buf(),
            db_key: [4u8; 32],
            default_server_url: String::new(),
            platform: "test".into(),
            events: events.clone(),
        })
        .unwrap();
        core.run(|i| async move {
            let st = i.get_media_retention();
            assert_eq!((st.days, st.max_days, st.options.clone()), (30, 30, vec![1, 7, 14, 30]));
            assert!(st.last_check.is_none());
            for bad in [0, 2, 31, 90] {
                let e = i.set_media_retention(bad).await.unwrap_err();
                assert_eq!(e.code, ErrorCode::InvalidInput);
            }
            assert_eq!(i.media_retention_days(), 30, "a refused value changes nothing");
            let st = i.set_media_retention(7).await?;
            assert_eq!(st.days, 7);
            assert!(st.last_check.is_some(), "changing it runs the check at once");
            assert_eq!(i.media_retention_limit_ms(), 7 * day_ms());

            // Another device: older change loses, newer wins, invalid values are ignored.
            let local = i.media_retention_record();
            i.apply_synced_media_retention(MediaRetentionRecord { days: 1, updated_at: local.updated_at - 1 })?;
            assert_eq!(i.media_retention_days(), 7);
            i.apply_synced_media_retention(MediaRetentionRecord { days: 365, updated_at: local.updated_at + 1 })?;
            assert_eq!(i.media_retention_days(), 7, "a period longer than 30 days is never accepted");
            i.apply_synced_media_retention(MediaRetentionRecord { days: 14, updated_at: local.updated_at + 2 })?;
            assert_eq!(i.media_retention_days(), 14);
            // A damaged stored value counts as 30 days, not longer.
            i.db.borrow().kv_set_json(KV_SETTING, &MediaRetentionRecord { days: 1000, updated_at: 1 })?;
            assert_eq!(i.media_retention_days(), 30);
            Ok(())
        })
        .await
        .unwrap();
        let changes = events.0.lock().unwrap().iter().filter(|e| *e == "media_retention_changed").count();
        assert_eq!(changes, 2, "the local change and the synced one");
    }

    #[tokio::test]
    async fn media_check_removes_strays_and_keeps_live_files() {
        let dir = tempfile::tempdir().unwrap();
        struct Nothing;
        impl crate::engine::EventSink for Nothing {
            fn emit(&self, _: &str, _: serde_json::Value) {}
        }
        let core = crate::engine::Core::start(crate::engine::CoreConfig {
            data_dir: dir.path().to_path_buf(),
            db_key: [3u8; 32],
            default_server_url: String::new(),
            platform: "test".into(),
            events: std::sync::Arc::new(Nothing),
        })
        .unwrap();
        let media = dir.path().join("media");
        let old = SystemTime::now() - std::time::Duration::from_secs(600);
        let plant = |name: &str, age: Option<SystemTime>| {
            let p = media.join(name);
            std::fs::write(&p, b"x").unwrap();
            if let Some(t) = age {
                std::fs::File::options().write(true).open(&p).unwrap().set_modified(t).unwrap();
            }
            p
        };
        let stray = plant("stray.jpg", Some(old));
        let part = plant("half.part", Some(old));
        let fresh = plant("downloading.jpg", None);
        let check = core.run(|i| async move { i.run_media_check().await }).await.unwrap();
        assert_eq!(check.removed, 2);
        assert!(!stray.exists() && !part.exists());
        assert!(fresh.exists(), "a file being written right now stays");
        let last = core.run(|i| async move { Ok(i.get_media_retention().last_check) }).await.unwrap();
        assert_eq!(last, Some(check));
    }

    #[test]
    fn stubs_without_expired_at_are_not_written() {
        // The SQL filter looks for "expired_at" in the JSON: live attachments must not carry it.
        let json = serde_json::to_string(&full()).unwrap();
        assert!(!json.contains("expired_at"));
        let old: StoredAttachment = serde_json::from_str(r#"{"id":"x","mime":"a/b","file_name":"","size":1,"width":null,"height":null,"duration_ms":null,"waveform":null,"thumbnail":null,"local_path":null,"download_state":"done","cdn_id":null,"key":null,"digest":null}"#).unwrap();
        assert!(old.expired_at.is_none());
    }
}
