//! Encryption of media files at rest (received and sent attachments, avatars).
//!
//! File layout: `MAGIC (8) || salt (16) || plaintext length (u64 LE)` followed by the plaintext
//! in 64 KiB chunks, each sealed with AES-256-GCM (+16 bytes). The file key is derived from
//! the media master key (kept in the password-locked database) and the salt. Chunk `i` uses
//! the nonce `0u32 || i (u64 BE)` and authenticates the header, its index and whether it is the
//! last chunk, so chunks cannot be reordered, swapped between files or cut off. Chunks allow
//! reading a byte range (audio/video seeking) without decrypting the whole file.

use std::io::{Read, Seek, SeekFrom};

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use hkdf::Hkdf;
use sha2::Sha256;

use crate::crypto::random_bytes;
use crate::error::{CoreError, Result};

pub const MAGIC: &[u8; 8] = b"CAMEDIA1";
pub const HEADER_LEN: usize = 8 + 16 + 8;
const CHUNK: usize = 64 * 1024;
const TAG: usize = 16;

fn file_cipher(master: &[u8; 32], salt: &[u8]) -> Aes256Gcm {
    let hk = Hkdf::<Sha256>::new(Some(salt), master);
    let mut key = [0u8; 32];
    hk.expand(b"chatapp/media-file/v1", &mut key).expect("valid length");
    Aes256Gcm::new_from_slice(&key).expect("32-byte key")
}

fn nonce(index: u64) -> [u8; 12] {
    let mut n = [0u8; 12];
    n[4..].copy_from_slice(&index.to_be_bytes());
    n
}

fn aad(header: &[u8], index: u64, last: bool) -> Vec<u8> {
    let mut a = Vec::with_capacity(header.len() + 9);
    a.extend_from_slice(header);
    a.extend_from_slice(&index.to_be_bytes());
    a.push(last as u8);
    a
}

fn chunk_count(len: u64) -> u64 {
    len.div_ceil(CHUNK as u64).max(1)
}

/// True when `head` (the first bytes of a file) is an encrypted media file.
pub fn is_encrypted(head: &[u8]) -> bool {
    head.len() >= HEADER_LEN && &head[..8] == MAGIC
}

pub fn encrypt(master: &[u8; 32], plain: &[u8]) -> Vec<u8> {
    let salt = random_bytes::<16>();
    let mut header = Vec::with_capacity(HEADER_LEN);
    header.extend_from_slice(MAGIC);
    header.extend_from_slice(&salt);
    header.extend_from_slice(&(plain.len() as u64).to_le_bytes());
    let cipher = file_cipher(master, &salt);
    let n = chunk_count(plain.len() as u64);
    let mut out = Vec::with_capacity(HEADER_LEN + plain.len() + n as usize * TAG);
    out.extend_from_slice(&header);
    for i in 0..n {
        let start = (i as usize) * CHUNK;
        let end = (start + CHUNK).min(plain.len());
        let ct = cipher
            .encrypt(Nonce::from_slice(&nonce(i)), Payload { msg: &plain[start..end], aad: &aad(&header, i, i + 1 == n) })
            .expect("encryption cannot fail");
        out.extend_from_slice(&ct);
    }
    out
}

/// Plaintext length recorded in the header.
pub fn plain_len(header: &[u8]) -> Result<u64> {
    if !is_encrypted(header) {
        return Err(CoreError::internal("not an encrypted media file"));
    }
    Ok(u64::from_le_bytes(header[24..32].try_into().expect("8 bytes")))
}

/// Decrypts the plaintext bytes `start..end` (end exclusive) from an encrypted file.
pub fn decrypt_range<R: Read + Seek>(master: &[u8; 32], file: &mut R, start: u64, end: u64) -> Result<Vec<u8>> {
    let mut header = [0u8; HEADER_LEN];
    file.seek(SeekFrom::Start(0))?;
    file.read_exact(&mut header)?;
    let len = plain_len(&header)?;
    let end = end.min(len);
    if start >= end {
        return Ok(Vec::new());
    }
    let cipher = file_cipher(master, &header[8..24]);
    let n = chunk_count(len);
    let (first, last) = (start / CHUNK as u64, (end - 1) / CHUNK as u64);
    let mut out = Vec::with_capacity((end - start) as usize);
    let mut buf = vec![0u8; CHUNK + TAG];
    for i in first..=last {
        let chunk_plain = if i + 1 == n { len - i * CHUNK as u64 } else { CHUNK as u64 } as usize;
        let ct = &mut buf[..chunk_plain + TAG];
        file.seek(SeekFrom::Start(HEADER_LEN as u64 + i * (CHUNK + TAG) as u64))?;
        file.read_exact(ct)?;
        let plain = cipher
            .decrypt(Nonce::from_slice(&nonce(i)), Payload { msg: ct, aad: &aad(&header, i, i + 1 == n) })
            .map_err(|_| CoreError::internal("media file is corrupt or was tampered with"))?;
        let chunk_start = i * CHUNK as u64;
        let from = start.saturating_sub(chunk_start) as usize;
        let to = ((end - chunk_start) as usize).min(plain.len());
        out.extend_from_slice(&plain[from..to]);
    }
    Ok(out)
}

pub fn decrypt(master: &[u8; 32], data: &[u8]) -> Result<Vec<u8>> {
    let len = plain_len(data)?;
    decrypt_range(master, &mut std::io::Cursor::new(data), 0, len)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_ranges() {
        let key = random_bytes::<32>();
        for size in [0usize, 1, CHUNK - 1, CHUNK, CHUNK + 1, 3 * CHUNK + 17] {
            let plain: Vec<u8> = (0..size).map(|i| (i * 7 % 251) as u8).collect();
            let enc = encrypt(&key, &plain);
            assert!(is_encrypted(&enc));
            assert!(!enc.windows(16.min(size.max(1))).any(|w| size >= 16 && w == &plain[..16]), "plaintext visible");
            assert_eq!(decrypt(&key, &enc).unwrap(), plain, "size {size}");
            for (s, e) in [(0, 1), (5, 100), (CHUNK as u64 - 3, CHUNK as u64 + 3), (0, size as u64), (size as u64 / 2, size as u64 + 50)] {
                let got = decrypt_range(&key, &mut std::io::Cursor::new(&enc), s, e).unwrap();
                let (s, e) = (s.min(size as u64) as usize, e.min(size as u64) as usize);
                assert_eq!(got, plain[s.min(e)..e], "size {size} range {s}..{e}");
            }
        }
    }

    #[test]
    fn tampering_and_wrong_key_are_detected() {
        let key = random_bytes::<32>();
        let plain = vec![9u8; 2 * CHUNK + 5];
        let mut enc = encrypt(&key, &plain);
        assert!(decrypt(&random_bytes::<32>(), &enc).is_err());
        let truncated = enc[..HEADER_LEN + CHUNK + TAG].to_vec();
        assert!(decrypt(&key, &truncated).is_err());
        let last = enc.len() - 1;
        enc[last] ^= 1;
        assert!(decrypt(&key, &enc).is_err());
    }
}
