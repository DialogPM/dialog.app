//! Attachment preparation, encrypted upload and download.

use std::io::Cursor;
use std::path::{Path, PathBuf};

use image::GenericImageView;

use super::{retention, Inner};
use crate::content::{AttachmentPointer, AvatarRef};
use crate::crypto::{self, b64, unb64};
use crate::error::{Context, CoreError, ErrorCode, Result};
use crate::model::{OutgoingAttachment, StoredAttachment};
use crate::sanitize;

/// Local upper bound; the server may enforce a lower one.
const MAX_ATTACHMENT: usize = 100 * 1024 * 1024;
const AVATAR_DIM: u32 = 512;

fn safe_extension(file_name: &str, mime: &str) -> String {
    let from_name = Path::new(file_name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .filter(|e| !e.is_empty() && e.len() <= 8 && e.chars().all(|c| c.is_ascii_alphanumeric()));
    from_name
        .or_else(|| mime_guess::get_mime_extensions_str(mime).and_then(|v| v.first()).map(|e| e.to_string()))
        .unwrap_or_else(|| "bin".into())
}

/// A file name from someone else: no path, no characters that are special in file systems,
/// no invisible / bidi characters (which could make "x\u{202E}fdp.exe" look like "xexe.pdf").
fn sanitize_file_name(name: &str) -> String {
    let base = Path::new(name).file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let cleaned: String = base
        .chars()
        .filter(|c| !sanitize::is_invisible_format_char(*c))
        .map(|c| if c.is_control() || "/\\:*?\"<>|".contains(c) { '_' } else { c })
        .take(200)
        .collect();
    if cleaned.trim().is_empty() {
        "file".into()
    } else {
        cleaned
    }
}

/// The file name with the extension of `mime` (after a photo was re-encoded).
fn with_extension_for(file_name: &str, mime: &str) -> String {
    let ext = match mime {
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/gif" => "gif",
        _ => return file_name.to_string(),
    };
    let stem = Path::new(file_name).file_stem().and_then(|s| s.to_str()).filter(|s| !s.is_empty()).unwrap_or("image");
    format!("{stem}.{ext}")
}

fn resize_avatar(bytes: &[u8]) -> Result<Vec<u8>> {
    let img = image::load_from_memory(bytes).map_err(|e| CoreError::invalid(format!("not an image: {e}")))?;
    let (w, h) = img.dimensions();
    // Center-crop to a square, then downscale.
    let side = w.min(h);
    let cropped = img.crop_imm((w - side) / 2, (h - side) / 2, side, side);
    let resized = cropped.resize_exact(AVATAR_DIM.min(side), AVATAR_DIM.min(side), image::imageops::FilterType::Lanczos3);
    let mut out = Cursor::new(Vec::new());
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 88)
        .encode_image(&resized.to_rgb8())
        .map_err(|e| CoreError::internal(e.to_string()))?;
    Ok(out.into_inner())
}

impl Inner {
    fn media_path(&self, name: &str) -> PathBuf {
        self.media_dir().join(name)
    }

    pub async fn prepare_outgoing_attachment(
        &self,
        message_id: &str,
        index: usize,
        a: OutgoingAttachment,
    ) -> Result<StoredAttachment> {
        let bytes = match (&a.path, &a.data_base64) {
            (Some(p), _) => {
                let meta = tokio::fs::metadata(p).await?;
                if meta.len() as usize > MAX_ATTACHMENT {
                    return Err(CoreError::new(ErrorCode::PayloadTooLarge, "file is larger than 100 MB"));
                }
                tokio::fs::read(p).await?
            }
            (None, Some(d)) => {
                let d = d.split_once("base64,").map(|(_, rest)| rest).unwrap_or(d);
                unb64(d)?
            }
            (None, None) => return Err(CoreError::invalid("attachment has no data")),
        };
        if bytes.is_empty() {
            return Err(CoreError::invalid("attachment is empty"));
        }
        if bytes.len() > MAX_ATTACHMENT {
            return Err(CoreError::new(ErrorCode::PayloadTooLarge, "file is larger than 100 MB"));
        }
        let mut mime = if a.mime.is_empty() {
            mime_guess::from_path(&a.file_name).first_or_octet_stream().to_string()
        } else {
            a.mime.clone()
        };
        let mut kind = match a.kind.as_str() {
            "image" | "video" | "voice" | "file" => a.kind.clone(),
            _ => "file".into(),
        };
        let mut data = bytes;
        let (mut width, mut height, mut thumbnail) = (a.width, a.height, None);
        let mut file_name = sanitize_file_name(&a.file_name);
        if kind == "image" {
            // Never the original bytes: metadata (EXIF GPS, ...) is removed here.
            let processed = tokio::task::spawn_blocking(move || {
                let p = sanitize::process_outgoing_image(&data);
                (data, p)
            })
            .await
            .ctx("image task")?;
            match processed {
                (_, Some(p)) => {
                    data = p.bytes;
                    if p.mime != mime {
                        file_name = with_extension_for(&file_name, &p.mime);
                    }
                    mime = p.mime;
                    width = Some(p.width as i64);
                    height = Some(p.height as i64);
                    thumbnail = Some(p.thumbnail);
                }
                (orig, None) => {
                    data = orig;
                    kind = "file".into(); // not decodable: send as a plain file
                }
            }
        }
        let ext = safe_extension(&file_name, &mime);
        let path = self.attachment_dir().join(format!("{message_id}-{index}.{ext}"));
        let size = data.len() as i64;
        self.write_media_file(&path, data).await?;
        Ok(StoredAttachment {
            id: format!("{message_id}-{index}"),
            mime,
            file_name,
            size,
            width,
            height,
            duration_ms: a.duration_ms,
            waveform: a.waveform.map(|w| w.into_iter().take(64).map(|v| v.clamp(0.0, 1.0)).collect()),
            thumbnail,
            local_path: Some(path.to_string_lossy().to_string()),
            download_state: "done".into(),
            cdn_id: None,
            key: None,
            digest: None,
            kind,
            expired_at: None,
        })
    }

    /// Uploads not-yet-uploaded attachments of a message and returns the wire pointers.
    pub async fn upload_message_attachments(&self, message_id: &str) -> Result<Vec<AttachmentPointer>> {
        let Some(row) = self.message_row(message_id)? else { return Ok(vec![]) };
        let mut atts = row.attachments.clone();
        let api = self.session()?.api.clone();
        for i in 0..atts.len() {
            // Erased by the retention: nothing left to send (engine/retention.rs).
            if atts[i].cdn_id.is_some() || retention::is_expired(&atts[i]) {
                continue;
            }
            let path = atts[i].local_path.clone().ok_or_else(|| CoreError::internal("attachment file missing"))?;
            let plain = self.read_media_file(Path::new(&path)).await?;
            let cap = self.max_attachment_bytes();
            let enc = tokio::task::spawn_blocking(move || crypto::encrypt_attachment_padded(&plain, cap)).await.ctx("encrypt task")?;
            let id = api.upload_attachment(enc.ciphertext, "message").await?;
            atts[i].cdn_id = Some(id);
            atts[i].key = Some(b64(enc.key));
            atts[i].digest = Some(b64(enc.digest));
            // Persist after each upload so a retry does not upload again.
            self.update_attachments(message_id, &atts)?;
        }
        let atts = self.message_row(message_id)?.map(|r| r.attachments).unwrap_or(atts);
        Ok(atts
            .iter()
            .filter_map(|a| {
                Some(AttachmentPointer {
                    cdn_id: a.cdn_id.clone()?,
                    key: a.key.clone()?,
                    digest: a.digest.clone()?,
                    size: a.size,
                    mime: a.mime.clone(),
                    file_name: a.file_name.clone(),
                    kind: a.kind.clone(),
                    width: a.width,
                    height: a.height,
                    duration_ms: a.duration_ms,
                    waveform: a.waveform.clone(),
                    thumbnail: a.thumbnail.clone(),
                })
            })
            .collect())
    }

    pub fn stored_from_pointer(&self, message_id: &str, index: usize, p: &AttachmentPointer) -> StoredAttachment {
        let thumbnail = p.thumbnail.clone().filter(|t| sanitize::is_safe_thumbnail(t));
        // Only image types the apps can safely render inline keep an image mime type (no
        // SVG, TIFF, ...); the real type is checked again after the download.
        let mut mime: String = p.mime.trim().to_ascii_lowercase().chars().take(100).collect();
        if !mime.chars().all(|c| c.is_ascii_alphanumeric() || "/.+-_".contains(c)) {
            mime = "application/octet-stream".into();
        }
        let mut kind = match p.kind.as_str() {
            "image" | "video" | "voice" | "file" => p.kind.clone(),
            _ => "file".into(),
        };
        if (mime.starts_with("image/") || kind == "image") && !sanitize::is_allowed_image_mime(&mime) {
            mime = "application/octet-stream".into();
            if kind == "image" {
                kind = "file".into();
            }
        }
        StoredAttachment {
            id: format!("{message_id}-{index}"),
            mime,
            file_name: sanitize_file_name(&p.file_name),
            size: p.size.max(0),
            width: p.width,
            height: p.height,
            duration_ms: p.duration_ms,
            waveform: p.waveform.clone().map(|w| w.into_iter().take(64).map(|v| v.clamp(0.0, 1.0)).collect()),
            thumbnail,
            local_path: None,
            download_state: "pending".into(),
            cdn_id: Some(p.cdn_id.clone()),
            key: Some(p.key.clone()),
            digest: Some(p.digest.clone()),
            kind,
            expired_at: None,
        }
    }

    /// Records the result of checking a downloaded image: its real type and size, or (not an
    /// allowed image, or too large to decode safely) shown as a plain file from now on.
    fn apply_image_check(&self, message_id: &str, att_id: &str, check: Option<(&'static str, u32, u32)>) -> Result<()> {
        let Some(row) = self.message_row(message_id)? else { return Ok(()) };
        let mut atts = row.attachments;
        let mut first_blocked = false;
        for (i, a) in atts.iter_mut().enumerate().filter(|(_, a)| a.id == att_id) {
            match check {
                Some((mime, w, h)) => {
                    a.mime = mime.to_string();
                    a.width = Some(w as i64);
                    a.height = Some(h as i64);
                }
                None => {
                    log::warn!("attachment {att_id} is not a safe image: shown as a file");
                    a.mime = "application/octet-stream".into();
                    a.kind = "file".into();
                    a.thumbnail = None;
                    a.width = None;
                    a.height = None;
                    first_blocked = i == 0;
                }
            }
        }
        self.update_attachments(message_id, &atts)?;
        if first_blocked {
            self.db
                .borrow()
                .conn
                .execute("UPDATE messages SET kind = 'file' WHERE id = ?1 AND kind = 'image'", [message_id])?;
        }
        Ok(())
    }

    fn set_attachment_state(&self, message_id: &str, att_id: &str, state: &str, path: Option<String>) -> Result<()> {
        if let Some(row) = self.message_row(message_id)? {
            let mut atts = row.attachments;
            for a in atts.iter_mut().filter(|a| a.id == att_id) {
                a.download_state = state.to_string();
                if path.is_some() {
                    a.local_path = path.clone();
                }
            }
            self.update_attachments(message_id, &atts)?;
            self.emit_message(message_id, false);
        }
        Ok(())
    }

    pub async fn download_attachment(&self, message_id: &str, att_id: &str) -> Result<()> {
        let row = self.message_row(message_id)?.ok_or_else(|| CoreError::not_found("message not found"))?;
        let a = row
            .attachments
            .iter()
            .find(|a| a.id == att_id)
            .cloned()
            .ok_or_else(|| CoreError::not_found("attachment not found"))?;
        // Past the 30-day retention: never fetched (again), even if the server still had it.
        if retention::is_expired(&a) || self.media_past_retention(row.sent_at, row.received_at) {
            return Err(retention::expired_error());
        }
        if a.download_state == "done" && a.local_path.as_ref().map(|p| Path::new(p).exists()).unwrap_or(false) {
            return Ok(());
        }
        if a.download_state == "downloading" {
            return Ok(());
        }
        let (Some(cdn), Some(key), Some(digest)) = (a.cdn_id.clone(), a.key.clone(), a.digest.clone()) else {
            return Err(CoreError::internal("attachment has no remote copy"));
        };
        self.set_attachment_state(message_id, att_id, "downloading", None)?;
        let declared_image = a.kind == "image" || a.mime.starts_with("image/");
        let result: Result<(String, Option<Option<(&'static str, u32, u32)>>)> = async {
            let api = self.session()?.api.clone();
            let padded = crypto::padded_attachment_size(a.size.max(0) as usize, usize::MAX);
            let data = api.download_attachment(&cdn, padded + 1024 * 1024).await?;
            let key = unb64(&key)?;
            let digest = unb64(&digest)?;
            let mut plain = tokio::task::spawn_blocking(move || crypto::decrypt_attachment(&key, &digest, &data))
                .await
                .ctx("decrypt task")??;
            // Cut the size padding off (the real size travels inside the encrypted message).
            if a.size >= 0 && plain.len() > a.size as usize {
                plain.truncate(a.size as usize);
            }
            // Before any app decodes it: the real type by its magic bytes (not the sender's
            // word) and the dimensions from the header only (decompression bombs).
            let (plain, check) = if declared_image {
                let (plain, check) = tokio::task::spawn_blocking(move || {
                    let c = sanitize::inspect_image(&plain, sanitize::MAX_INCOMING_SIDE, sanitize::MAX_INCOMING_PIXELS);
                    (plain, c)
                })
                .await
                .ctx("image check task")?;
                (plain, Some(check))
            } else {
                (plain, None)
            };
            let ext = match check {
                Some(Some((mime, _, _))) => mime.trim_start_matches("image/").replace("jpeg", "jpg"),
                Some(None) => "bin".into(),
                None => safe_extension(&a.file_name, &a.mime),
            };
            // Defense in depth: local file names only from safe characters.
            if att_id.is_empty() || !att_id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
                return Err(CoreError::invalid("invalid attachment id"));
            }
            let path = self.attachment_dir().join(format!("{att_id}.{ext}"));
            self.write_media_file(&path, plain).await?;
            Ok((path.to_string_lossy().to_string(), check))
        }
        .await;
        match result {
            Ok((path, check)) => {
                if let Some(check) = check {
                    self.apply_image_check(message_id, att_id, check)?;
                }
                self.set_attachment_state(message_id, att_id, "done", Some(path))?;
                Ok(())
            }
            Err(e) => {
                let state = if e.code == ErrorCode::NotFound { "failed" } else { "pending" };
                self.set_attachment_state(message_id, att_id, state, None)?;
                Err(e)
            }
        }
    }

    pub async fn save_attachment(&self, message_id: &str, att_id: &str, dest: &str) -> Result<()> {
        let row = self.message_row(message_id)?.ok_or_else(|| CoreError::not_found("message not found"))?;
        if row.view_once != 0 {
            return Err(CoreError::invalid("view-once media cannot be saved"));
        }
        let a = row.attachments.iter().find(|a| a.id == att_id).ok_or_else(|| CoreError::not_found("attachment"))?;
        if self.message_media_expired(&row) {
            return Err(retention::expired_error());
        }
        let src = a.local_path.clone().ok_or_else(|| CoreError::invalid("download the attachment first"))?;
        // The user's export: the only place a decrypted copy is written, where they chose.
        self.export_media_file(Path::new(&src), Path::new(dest)).await
    }

    /// Encrypts and uploads an avatar image; returns (local path, reference).
    pub async fn upload_avatar_from_path(&self, path: &str, tag: &str) -> Result<(String, AvatarRef)> {
        let bytes = self.read_media_file(Path::new(path)).await?;
        if bytes.len() > 20 * 1024 * 1024 {
            return Err(CoreError::invalid("image is too large"));
        }
        let jpeg = tokio::task::spawn_blocking(move || resize_avatar(&bytes)).await.ctx("avatar task")??;
        let local = self.media_path(&format!("avatar-{tag}-{}.jpg", &crate::util::new_id()[..8]));
        self.write_media_file(&local, jpeg.clone()).await?;
        let enc = crypto::encrypt_attachment(&jpeg);
        let id = self.session()?.api.upload_attachment(enc.ciphertext, "avatar").await?;
        Ok((
            local.to_string_lossy().to_string(),
            AvatarRef { cdn_id: id, key: b64(enc.key), digest: b64(enc.digest) },
        ))
    }

    pub async fn download_avatar(&self, a: &AvatarRef, tag: &str) -> Result<String> {
        let api = self.session()?.api.clone();
        let data = api.download_attachment(&a.cdn_id, 8 * 1024 * 1024).await?;
        let plain = crypto::decrypt_attachment(&unb64(&a.key)?, &unb64(&a.digest)?, &data)?;
        // Only accept real images of a sane size.
        sanitize::inspect_image(&plain, 4096, 4096 * 4096).ok_or_else(|| CoreError::internal("avatar is not an image"))?;
        let safe_tag: String = tag.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').take(64).collect();
        let local = self.media_path(&format!("avatar-{safe_tag}-{}.jpg", &a.cdn_id[..8.min(a.cdn_id.len())]));
        self.write_media_file(&local, plain).await?;
        Ok(local.to_string_lossy().to_string())
    }

    /// The server's attachment size limit (padding never goes above it).
    fn max_attachment_bytes(&self) -> usize {
        let v: Option<i64> = self.db.borrow().kv_get_json("max_attachment_bytes").ok().flatten();
        v.filter(|v| *v > 0).map(|v| v as usize).unwrap_or(100 * 1024 * 1024)
    }
}
