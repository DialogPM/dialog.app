//! Media files at rest: encrypted with a key kept in the (password-locked) database, read
//! back decrypted for the UI, and removed when nothing references them any more.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use super::Inner;
use crate::crypto::{self, b64};
use crate::error::{Context, CoreError, Result};
use crate::media_crypto;

/// Files younger than this are never swept (a download or a send may be writing them).
pub(super) const SWEEP_MIN_AGE: Duration = Duration::from_secs(120);

impl Inner {
    /// The media master key (random, created on first use, stored in the database).
    pub fn media_key(&self) -> Result<[u8; 32]> {
        let db = self.db.borrow();
        if let Some(k) = db.kv_get("media_key")? {
            return crypto::key32(&k);
        }
        let k = crypto::random_bytes::<32>();
        db.kv_set("media_key", &k)?;
        Ok(k)
    }

    /// True for paths inside our media directories (the only files we encrypt and delete).
    pub fn is_own_media(&self, path: &Path) -> bool {
        // `starts_with` compares components without resolving "..": "media/../db.key" would
        // pass. Refuse such paths, and compare resolved paths when the file exists.
        if path.components().any(|c| matches!(c, std::path::Component::ParentDir | std::path::Component::CurDir)) {
            return false;
        }
        let inside = |base: PathBuf| {
            match (path.canonicalize(), base.canonicalize()) {
                (Ok(p), Ok(b)) => p.starts_with(b),
                _ => path.starts_with(&base),
            }
        };
        inside(self.media_dir()) || inside(self.temp_media_dir())
    }

    /// Encrypts `plain` into `path` (atomically).
    pub async fn write_media_file(&self, path: &Path, plain: Vec<u8>) -> Result<()> {
        let key = self.media_key()?;
        let path = path.to_path_buf();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let enc = media_crypto::encrypt(&key, &plain);
            let part = path.with_extension("part");
            std::fs::write(&part, enc)?;
            std::fs::rename(&part, &path)?;
            Ok(())
        })
        .await
        .ctx("media write task")?
    }

    /// Reads a media file decrypted. Files that are not encrypted (outside our directories, or
    /// not migrated yet) are returned as they are.
    pub async fn read_media_file(&self, path: &Path) -> Result<Vec<u8>> {
        let data = tokio::fs::read(path).await?;
        if !media_crypto::is_encrypted(&data) {
            return Ok(data);
        }
        let key = self.media_key()?;
        tokio::task::spawn_blocking(move || media_crypto::decrypt(&key, &data)).await.ctx("media read task")?
    }

    /// Decrypted bytes `start..end` (end exclusive, clamped) and the total plaintext size.
    pub async fn read_media_range(&self, path: &Path, start: u64, end: Option<u64>) -> Result<(Vec<u8>, u64)> {
        let key = self.media_key()?;
        let path = path.to_path_buf();
        tokio::task::spawn_blocking(move || -> Result<(Vec<u8>, u64)> {
            let mut f = std::fs::File::open(&path)?;
            let mut head = [0u8; media_crypto::HEADER_LEN];
            let n = std::io::Read::read(&mut f, &mut head)?;
            if !media_crypto::is_encrypted(&head[..n]) {
                use std::io::{Read, Seek, SeekFrom};
                let len = f.metadata()?.len();
                let end = end.unwrap_or(len).min(len);
                let mut buf = vec![0u8; end.saturating_sub(start) as usize];
                f.seek(SeekFrom::Start(start))?;
                f.read_exact(&mut buf)?;
                return Ok((buf, len));
            }
            let len = media_crypto::plain_len(&head)?;
            let data = media_crypto::decrypt_range(&key, &mut f, start, end.unwrap_or(len))?;
            Ok((data, len))
        })
        .await
        .ctx("media range task")?
    }

    /// Writes a decrypted copy of one of our media files to `dest` (export, sharing, players
    /// that need a file).
    pub async fn export_media_file(&self, path: &Path, dest: &Path) -> Result<()> {
        if !self.is_own_media(path) {
            return Err(CoreError::invalid("not a media file of this app"));
        }
        let plain = self.read_media_file(path).await?;
        tokio::fs::write(dest, plain).await?;
        Ok(())
    }

    /// Encrypts media files left unencrypted by older versions (once per start).
    pub async fn encrypt_legacy_media(&self) -> Result<usize> {
        let key = self.media_key()?;
        let dirs = [self.media_dir(), self.temp_media_dir()];
        tokio::task::spawn_blocking(move || -> Result<usize> {
            let mut done = 0;
            for dir in dirs {
                let Ok(entries) = std::fs::read_dir(&dir) else { continue };
                for e in entries.flatten() {
                    let p = e.path();
                    if !p.is_file() || p.extension().is_some_and(|x| x == "part") {
                        continue;
                    }
                    let data = std::fs::read(&p)?;
                    if media_crypto::is_encrypted(&data) {
                        continue;
                    }
                    let part = p.with_extension("part");
                    std::fs::write(&part, media_crypto::encrypt(&key, &data))?;
                    std::fs::rename(&part, &p)?;
                    done += 1;
                }
            }
            Ok(done)
        })
        .await
        .ctx("media migration task")?
    }

    /// Every media path still referenced by a message, a contact, a group or our profile.
    fn referenced_media(&self) -> Result<HashSet<PathBuf>> {
        let mut out = self.referenced_message_media()?;
        out.extend(self.referenced_avatars()?);
        Ok(out)
    }

    /// Media paths used by message attachments (erased ones have none).
    pub(super) fn referenced_message_media(&self) -> Result<HashSet<PathBuf>> {
        let db = self.db.borrow();
        let mut out = HashSet::new();
        let mut attachments: Vec<String> = Vec::new();
        for table in ["main.messages", "temp.messages"] {
            let sql = format!("SELECT attachments_json FROM {table} WHERE attachments_json IS NOT NULL AND attachments_json != '[]'");
            if let Ok(mut st) = db.conn.prepare(&sql) {
                let rows: Vec<String> = st.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
                attachments.extend(rows);
            }
        }
        for json in attachments {
            if let Ok(list) = serde_json::from_str::<Vec<crate::model::StoredAttachment>>(&json) {
                out.extend(list.into_iter().filter_map(|a| a.local_path).map(PathBuf::from));
            }
        }
        Ok(out)
    }

    /// Avatar files (contacts, groups, our profile): not message media, never expired.
    pub(super) fn referenced_avatars(&self) -> Result<HashSet<PathBuf>> {
        let db = self.db.borrow();
        let mut out = HashSet::new();
        for sql in ["SELECT avatar_path FROM contacts WHERE avatar_path IS NOT NULL", "SELECT avatar_path FROM groups WHERE avatar_path IS NOT NULL"] {
            let mut st = db.conn.prepare(sql)?;
            let rows: Vec<String> = st.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
            out.extend(rows.into_iter().map(PathBuf::from));
        }
        if let Some(p) = db.kv_get_json::<crate::model::Profile>("profile")?.and_then(|p| p.avatar_path) {
            out.insert(PathBuf::from(p));
        }
        Ok(out)
    }

    /// Deletes media files nothing references any more (a download that finished after its
    /// message disappeared, replaced avatars, ...).
    pub fn sweep_orphan_media(&self) -> Result<usize> {
        let referenced = self.referenced_media()?;
        let now = SystemTime::now();
        let mut removed = 0;
        for dir in [self.media_dir(), self.temp_media_dir()] {
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            for e in entries.flatten() {
                let p = e.path();
                let old_enough = e
                    .metadata()
                    .and_then(|m| m.modified())
                    .map(|t| now.duration_since(t).unwrap_or_default() >= SWEEP_MIN_AGE)
                    .unwrap_or(false);
                if p.is_file() && old_enough && !referenced.contains(&p) {
                    let _ = std::fs::remove_file(&p);
                    removed += 1;
                }
            }
        }
        if removed > 0 {
            log::info!("removed {removed} unreferenced media files");
        }
        Ok(removed)
    }

    /// Whether another message (than `except`) still uses this file (forwarded copies share it).
    pub(super) fn media_shared(&self, path: &str, except: &str) -> bool {
        let needle = serde_json::to_string(path).unwrap_or_default();
        let needle = needle.trim_matches('"');
        self.db
            .borrow()
            .conn
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE id != ?1 AND instr(attachments_json, ?2) > 0",
                rusqlite::params![except, needle],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| n > 0)
            .unwrap_or(true)
    }

    /// Base64 of a decrypted media file (for the mobile app and the test bridge).
    pub async fn read_media_base64(&self, path: &str) -> Result<String> {
        let p = Path::new(path);
        if !self.is_own_media(p) {
            return Err(CoreError::invalid("not a media file of this app"));
        }
        Ok(b64(self.read_media_file(p).await?))
    }

    /// Encrypts files left by older versions. Unreferenced and too old files are removed by
    /// the media check (engine/retention.rs), run from the expiry loop.
    pub(super) async fn media_loop(self: std::rc::Rc<Self>) {
        match self.encrypt_legacy_media().await {
            Ok(0) => {}
            Ok(n) => log::info!("encrypted {n} media files left unencrypted by an older version"),
            Err(e) => log::warn!("media migration: {e}"),
        }
    }
}
