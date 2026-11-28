//! Exporting a conversation to a file the user chose: plain text, or a ZIP archive with the
//! text and the media of the conversation.
//!
//! The transcript is written by the UI (it has the user's language and already words every
//! kind of message); the core writes it out and adds the media files. The export leaves the
//! app unencrypted, so it only ever happens on the user's explicit request, and it never
//! holds more than the conversation shows: no view-once media, nothing of deleted or expired
//! messages, no media erased by the retention period.

use std::collections::HashSet;
use std::io::Write;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::Inner;
use crate::error::{Context, CoreError, Result};
use crate::util::now_ms;

/// Name of the transcript inside the archive when the UI gives none.
const DEFAULT_TEXT_NAME: &str = "chat.txt";
/// Folder of the media files inside the archive.
const MEDIA_DIR: &str = "media";

#[derive(Debug, Deserialize)]
pub struct ExportConversationRequest {
    pub conversation_id: String,
    /// Where the user chose to save the export.
    pub dest_path: String,
    /// The transcript, in the user's language.
    pub text: String,
    /// false: `dest_path` is the transcript itself. true: a ZIP archive holding the transcript
    /// and `media`.
    #[serde(default)]
    pub archive: bool,
    /// Name of the transcript inside the archive.
    #[serde(default)]
    pub text_name: Option<String>,
    /// The attachments to add to the archive, under the names the transcript refers to.
    #[serde(default)]
    pub media: Vec<ExportMedia>,
}

#[derive(Debug, Deserialize)]
pub struct ExportMedia {
    pub message_id: String,
    pub attachment_id: String,
    /// File name inside the archive's media folder (no path).
    pub name: String,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct ExportResult {
    pub media_exported: u32,
    /// Asked for but left out: not on this device, erased, view-once, deleted or expired.
    pub media_skipped: u32,
}

/// A file name that stays inside the archive's folder on every system.
fn safe_entry_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 240
        && name != "."
        && name != ".."
        && !name.chars().any(|c| c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'))
}

impl Inner {
    pub async fn export_conversation(&self, req: ExportConversationRequest) -> Result<ExportResult> {
        self.conversation_row(&req.conversation_id)?;
        let dest = Path::new(&req.dest_path).to_path_buf();
        if !req.archive {
            if !req.media.is_empty() {
                return Err(CoreError::invalid("media needs an archive"));
            }
            tokio::fs::write(&dest, req.text.as_bytes()).await?;
            return Ok(ExportResult { media_exported: 0, media_skipped: 0 });
        }
        let text_name = req.text_name.as_deref().unwrap_or(DEFAULT_TEXT_NAME);
        if !safe_entry_name(text_name) {
            return Err(CoreError::invalid("invalid file name"));
        }
        let mut names = HashSet::new();
        for m in &req.media {
            // Case-insensitive: the archive may be unpacked on a file system that is.
            if !safe_entry_name(&m.name) || !names.insert(m.name.to_lowercase()) {
                return Err(CoreError::invalid("invalid media file name"));
            }
        }
        let result = self.write_archive(&dest, text_name, &req).await;
        if result.is_err() {
            // Half an archive is of no use to anyone.
            let _ = tokio::fs::remove_file(&dest).await;
        }
        result
    }

    async fn write_archive(&self, dest: &Path, text_name: &str, req: &ExportConversationRequest) -> Result<ExportResult> {
        let now = now_ms();
        let mut zip = ZipWriter::create(dest)?;
        zip = add_entry(zip, text_name.to_string(), req.text.clone().into_bytes(), now).await?;
        let (mut exported, mut skipped) = (0, 0);
        for m in &req.media {
            let Some((path, sent_at)) = self.exportable_attachment(&req.conversation_id, m, now)? else {
                skipped += 1;
                continue;
            };
            // One file at a time in memory, decrypted only on its way into the archive.
            let Ok(plain) = self.read_media_file(Path::new(&path)).await else {
                skipped += 1;
                continue;
            };
            zip = add_entry(zip, format!("{MEDIA_DIR}/{}", m.name), plain, sent_at).await?;
            exported += 1;
        }
        tokio::task::spawn_blocking(move || zip.finish()).await.ctx("export task")??;
        Ok(ExportResult { media_exported: exported, media_skipped: skipped })
    }

    /// The decrypted-at-rest file of an attachment the conversation still shows, and when its
    /// message was sent. `None`: nothing to export for it.
    fn exportable_attachment(&self, conv_id: &str, m: &ExportMedia, now: i64) -> Result<Option<(String, i64)>> {
        let Some(row) = self.message_row(&m.message_id)? else { return Ok(None) };
        if row.conversation_id != conv_id {
            return Err(CoreError::invalid("the message is not in this conversation"));
        }
        if row.deleted || row.view_once != 0 || row.expires_at.is_some_and(|t| t <= now) || self.message_media_expired(&row) {
            return Ok(None);
        }
        let Some(path) = row.attachments.iter().find(|a| a.id == m.attachment_id).and_then(|a| a.local_path.clone()) else {
            return Ok(None);
        };
        if !self.is_own_media(Path::new(&path)) {
            return Ok(None);
        }
        Ok(Some((path, row.sent_at)))
    }
}

async fn add_entry(mut zip: ZipWriter, name: String, data: Vec<u8>, time_ms: i64) -> Result<ZipWriter> {
    tokio::task::spawn_blocking(move || {
        zip.add(&name, &data, time_ms)?;
        Ok(zip)
    })
    .await
    .ctx("export task")?
}

// --- a minimal ZIP writer ----------------------------------------------------------------
// Entries are stored, not compressed: photos, videos and voice notes are compressed already.
// The classic format: at most 65535 entries and 4 GiB in all (no ZIP64). Media is kept for 30
// days at most, so a conversation's media stays far below that.

struct ZipEntry {
    name: String,
    crc: u32,
    size: u32,
    offset: u32,
    dos_time: u16,
    dos_date: u16,
}

struct ZipWriter {
    file: std::io::BufWriter<std::fs::File>,
    entries: Vec<ZipEntry>,
    offset: u64,
}

/// Version 2.0, file names in UTF-8.
const ZIP_VERSION: u16 = 20;
const ZIP_UTF8: u16 = 1 << 11;

fn too_large() -> CoreError {
    CoreError::invalid("export_too_large")
}

impl ZipWriter {
    fn create(dest: &Path) -> Result<Self> {
        Ok(Self { file: std::io::BufWriter::new(std::fs::File::create(dest)?), entries: Vec::new(), offset: 0 })
    }

    fn add(&mut self, name: &str, data: &[u8], time_ms: i64) -> Result<()> {
        // Room is kept for the central directory, which repeats every name.
        let after = self.offset + 30 + name.len() as u64 + data.len() as u64;
        let directory: u64 = self.entries.iter().map(|e| 46 + e.name.len() as u64).sum::<u64>() + 46 + name.len() as u64 + 22;
        if self.entries.len() >= u16::MAX as usize - 1 || name.len() > u16::MAX as usize || after + directory > u32::MAX as u64 {
            return Err(too_large());
        }
        let (dos_date, dos_time) = dos_date_time(time_ms);
        let e = ZipEntry { name: name.to_string(), crc: crc32(data), size: data.len() as u32, offset: self.offset as u32, dos_time, dos_date };
        let mut h = Vec::with_capacity(30 + name.len());
        h.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        h.extend_from_slice(&ZIP_VERSION.to_le_bytes());
        h.extend_from_slice(&ZIP_UTF8.to_le_bytes());
        h.extend_from_slice(&0u16.to_le_bytes()); // stored
        h.extend_from_slice(&e.dos_time.to_le_bytes());
        h.extend_from_slice(&e.dos_date.to_le_bytes());
        h.extend_from_slice(&e.crc.to_le_bytes());
        h.extend_from_slice(&e.size.to_le_bytes());
        h.extend_from_slice(&e.size.to_le_bytes());
        h.extend_from_slice(&(name.len() as u16).to_le_bytes());
        h.extend_from_slice(&0u16.to_le_bytes());
        h.extend_from_slice(name.as_bytes());
        self.file.write_all(&h)?;
        self.file.write_all(data)?;
        self.offset = after;
        self.entries.push(e);
        Ok(())
    }

    fn finish(mut self) -> Result<()> {
        let start = self.offset;
        let mut dir = Vec::new();
        for e in &self.entries {
            dir.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
            dir.extend_from_slice(&ZIP_VERSION.to_le_bytes()); // made by
            dir.extend_from_slice(&ZIP_VERSION.to_le_bytes()); // needed
            dir.extend_from_slice(&ZIP_UTF8.to_le_bytes());
            dir.extend_from_slice(&0u16.to_le_bytes());
            dir.extend_from_slice(&e.dos_time.to_le_bytes());
            dir.extend_from_slice(&e.dos_date.to_le_bytes());
            dir.extend_from_slice(&e.crc.to_le_bytes());
            dir.extend_from_slice(&e.size.to_le_bytes());
            dir.extend_from_slice(&e.size.to_le_bytes());
            dir.extend_from_slice(&(e.name.len() as u16).to_le_bytes());
            dir.extend_from_slice(&[0u8; 12]); // extra, comment, disk, internal and external attributes
            dir.extend_from_slice(&e.offset.to_le_bytes());
            dir.extend_from_slice(e.name.as_bytes());
        }
        let count = self.entries.len() as u16;
        dir.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        dir.extend_from_slice(&[0u8; 4]); // disk numbers
        dir.extend_from_slice(&count.to_le_bytes());
        dir.extend_from_slice(&count.to_le_bytes());
        dir.extend_from_slice(&((dir.len() - 12) as u32).to_le_bytes());
        dir.extend_from_slice(&(start as u32).to_le_bytes());
        dir.extend_from_slice(&0u16.to_le_bytes());
        self.file.write_all(&dir)?;
        self.file.flush()?;
        Ok(())
    }
}

/// CRC-32 (IEEE), as ZIP wants it.
fn crc32(data: &[u8]) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut t = [0u32; 256];
        for (i, slot) in t.iter_mut().enumerate() {
            let mut c = i as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
            }
            *slot = c;
        }
        t
    });
    !data.iter().fold(!0u32, |c, b| table[((c ^ *b as u32) & 0xff) as usize] ^ (c >> 8))
}

/// MS-DOS date and time (UTC) of a timestamp in milliseconds; the format starts in 1980.
fn dos_date_time(ms: i64) -> (u16, u16) {
    let secs = ms.div_euclid(1000);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Days since 1970 to a calendar date (proleptic Gregorian).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    if !(1980..=2107).contains(&year) {
        return ((1 << 5) | 1, 0);
    }
    let date = (((year - 1980) as u16) << 9) | ((month as u16) << 5) | day as u16;
    let time = (((rem / 3600) as u16) << 11) | ((((rem % 3600) / 60) as u16) << 5) | ((rem % 60) / 2) as u16;
    (date, time)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u16_at(b: &[u8], i: usize) -> usize {
        u16::from_le_bytes([b[i], b[i + 1]]) as usize
    }
    fn u32_at(b: &[u8], i: usize) -> usize {
        u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]) as usize
    }

    /// Reads an archive back the way an unpacker does: from the central directory at the end.
    fn read_zip(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
        let end = bytes.len() - 22;
        assert_eq!(u32_at(bytes, end), 0x0605_4b50, "end of central directory");
        let (count, size, mut at) = (u16_at(bytes, end + 10), u32_at(bytes, end + 12), u32_at(bytes, end + 16));
        assert_eq!(at + size, end, "the directory ends where the end record starts");
        let mut out = Vec::new();
        for _ in 0..count {
            assert_eq!(u32_at(bytes, at), 0x0201_4b50);
            assert_eq!(u16_at(bytes, at + 10), 0, "stored");
            let (crc, len, name_len, local) = (u32_at(bytes, at + 16), u32_at(bytes, at + 24), u16_at(bytes, at + 28), u32_at(bytes, at + 42));
            let name = String::from_utf8(bytes[at + 46..at + 46 + name_len].to_vec()).unwrap();
            assert_eq!(u32_at(bytes, local), 0x0403_4b50);
            assert_eq!(&bytes[local + 30..local + 30 + name_len], name.as_bytes(), "the local header names the same file");
            let data = bytes[local + 30 + name_len..local + 30 + name_len + len].to_vec();
            assert_eq!(crc32(&data) as usize, crc);
            out.push((name, data));
            at += 46 + name_len;
        }
        assert_eq!(at, end);
        out
    }

    #[test]
    fn crc32_matches_the_reference_value() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn dos_dates() {
        // 2026-11-28 14:03:58 UTC
        let (date, time) = dos_date_time(1_795_874_638_000);
        assert_eq!((1980 + (date >> 9), (date >> 5) & 15, date & 31), (2026, 11, 28));
        assert_eq!((time >> 11, (time >> 5) & 63, (time & 31) * 2), (14, 3, 58));
        // Leap day, and a date before the format's first year.
        let (date, _) = dos_date_time(1_709_164_800_000);
        assert_eq!((1980 + (date >> 9), (date >> 5) & 15, date & 31), (2024, 2, 29));
        assert_eq!(dos_date_time(0), ((1 << 5) | 1, 0));
    }

    #[test]
    fn archive_reads_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chat.zip");
        let photo: Vec<u8> = (0..70_000u32).map(|i| (i * 7 % 251) as u8).collect();
        let mut zip = ZipWriter::create(&path).unwrap();
        zip.add("Conversație.txt", "[2026-11-28 14:03] Ana: salut 👋\n".as_bytes(), 1_795_874_638_000).unwrap();
        zip.add("media/0001-poză.jpg", &photo, 1_795_874_638_000).unwrap();
        zip.add("media/0002-empty.bin", &[], 1_795_874_638_000).unwrap();
        zip.finish().unwrap();
        let files = read_zip(&std::fs::read(&path).unwrap());
        assert_eq!(files.len(), 3);
        assert_eq!(files[0].0, "Conversație.txt");
        assert_eq!(String::from_utf8(files[0].1.clone()).unwrap(), "[2026-11-28 14:03] Ana: salut 👋\n");
        assert_eq!(files[1], ("media/0001-poză.jpg".to_string(), photo));
        assert_eq!(files[2], ("media/0002-empty.bin".to_string(), Vec::new()));
        // The system's own unpacker agrees, where there is one.
        if let Ok(out) = std::process::Command::new("unzip").arg("-tqq").arg(&path).output() {
            assert!(out.status.success(), "unzip -t: {}", String::from_utf8_lossy(&out.stdout));
        }
    }

    #[test]
    fn entry_names_stay_inside_the_archive() {
        for ok in ["chat.txt", "0001-photo.jpg", "Conversație cu Ana.txt", "..hidden"] {
            assert!(safe_entry_name(ok), "{ok}");
        }
        for bad in ["", ".", "..", "../x", "a/b", "a\\b", "C:x", "a\0b", "a\nb", "what?.txt"] {
            assert!(!safe_entry_name(bad), "{bad:?}");
        }
        assert!(!safe_entry_name(&"a".repeat(241)));
    }
}
