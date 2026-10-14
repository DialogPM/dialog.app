//! Username links: a shareable link / QR code that leads to this account's username without
//! carrying the account number.
//!
//! Design (same idea as Signal's username links, our own code):
//! - the client picks a random 32-byte key and encrypts its (zero-padded) username with
//!   AES-256-GCM; the server stores only the ciphertext under a random handle (UUID) it picks
//!   (`PUT /v1/account/username/link`);
//! - the link is `chatapp://u?s=<server>#<base64url(key || handle)>`: the key lives only in the
//!   fragment, so the server never learns which username a handle encrypts (it does know
//!   which account owns the handle);
//! - whoever opens the link fetches the blob by handle (`GET /v1/username/link/{handle}`),
//!   decrypts the username and resolves it in the directory like a typed `@username`;
//! - resetting creates a new handle (old links stop working); changing or removing the
//!   username deletes the link on the server.
//!
//! What still exposes the account number: the link itself does not contain it, so it can be
//! posted publicly. But resolving the username goes through the directory, whose entries
//! (`/v1/directory/username/..`, and `/v1/directory/account/{id}` used by `ensure_contact`
//! for anyone who has messaged you) include the number, because local contacts, contact
//! links and the vault are keyed / displayed by number. Hiding the number from
//! authenticated users who resolve the link would need number-less directory entries and
//! number-less contacts throughout the apps (a larger refactor), so it is not done here.

use std::rc::Rc;

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::Inner;
use crate::api::AccountDto;
use crate::crypto::{self, b64, unb64};
use crate::error::{CoreError, ErrorCode, Result};
use crate::model::{Contact, FoundAccount};
use crate::util::normalize_server_url;

const KV_KEY: &str = "username_link";
const AAD: &[u8] = b"chatapp-username-link-v1";
/// Usernames are at most 32 ASCII bytes; padding hides their length.
const PADDED_LEN: usize = 32;
const LINK_PREFIX: &str = "chatapp://u";

#[derive(Serialize, Deserialize, Clone)]
struct StoredLink {
    handle: String,
    key: String,
    username: String,
}

/// The account's username link (None when no username is set).
#[derive(Debug, Clone, Serialize)]
pub struct UsernameLink {
    pub link: String,
    pub username: String,
}

#[derive(Deserialize)]
struct HandleDto {
    handle: String,
}

#[derive(Deserialize)]
struct BlobDto {
    encrypted_username: String,
}

fn b64url() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
}

fn encrypt_username(key: &[u8; 32], username: &str) -> Result<Vec<u8>> {
    if username.len() > PADDED_LEN {
        return Err(CoreError::invalid("username too long"));
    }
    let mut padded = username.as_bytes().to_vec();
    padded.resize(PADDED_LEN, 0);
    crypto::aead_seal(key, &padded, AAD)
}

fn decrypt_username(key: &[u8; 32], blob: &[u8]) -> Result<String> {
    let plain = crypto::aead_open(key, blob, AAD).map_err(|_| CoreError::invalid("this username link is invalid"))?;
    let end = plain.iter().position(|b| *b == 0).unwrap_or(plain.len());
    let name = std::str::from_utf8(&plain[..end]).map_err(|_| CoreError::invalid("this username link is invalid"))?;
    if name.is_empty() || !name.is_ascii() {
        return Err(CoreError::invalid("this username link is invalid"));
    }
    Ok(name.to_string())
}

fn format_link(server_url: &str, key: &[u8; 32], handle: &str) -> Result<String> {
    let uuid = uuid::Uuid::parse_str(handle).map_err(|_| CoreError::internal("bad link handle"))?;
    let mut raw = Vec::with_capacity(48);
    raw.extend_from_slice(key);
    raw.extend_from_slice(uuid.as_bytes());
    let server: String = url::form_urlencoded::byte_serialize(server_url.as_bytes()).collect();
    Ok(format!("{LINK_PREFIX}?s={server}#{}", b64url().encode(raw)))
}

/// A parsed username link: (key, handle, server).
pub(crate) struct ParsedLink {
    key: [u8; 32],
    handle: String,
    server: Option<String>,
}

/// Whether `link` looks like a username link (as opposed to a number contact link).
pub(crate) fn is_username_link(link: &str) -> bool {
    let l = link.trim();
    l.starts_with("chatapp://u?") || l.starts_with("chatapp://u#")
}

pub(crate) fn parse_link(link: &str) -> Result<ParsedLink> {
    let bad = || CoreError::invalid("not a username link");
    let url = url::Url::parse(link.trim()).map_err(|_| bad())?;
    if url.scheme() != "chatapp" || url.host_str() != Some("u") {
        return Err(bad());
    }
    let server = url.query_pairs().find(|(k, _)| k == "s").map(|(_, v)| v.to_string());
    let raw = b64url().decode(url.fragment().ok_or_else(bad)?.trim()).map_err(|_| bad())?;
    if raw.len() != 48 {
        return Err(bad());
    }
    let key = crypto::key32(&raw[..32])?;
    let handle = uuid::Uuid::from_slice(&raw[32..]).map_err(|_| bad())?.hyphenated().to_string();
    Ok(ParsedLink { key, handle, server })
}

impl Inner {
    /// Returns the username link, creating one when there is none yet (or when the stored one
    /// no longer matches the server, e.g. after a reset on another device or a username
    /// change). None when the account has no username.
    pub async fn get_username_link(&self) -> Result<Option<UsernameLink>> {
        let s = self.session()?;
        let dto: AccountDto = s.api.get("/v1/account").await?;
        let Some(username) = dto.username else {
            return Ok(None);
        };
        let stored: Option<StoredLink> = self.db.borrow().kv_get_json(KV_KEY)?;
        if let Some(st) = stored.filter(|st| st.username == username) {
            let current: Result<HandleDto> = s.api.get("/v1/account/username/link").await;
            match current {
                Ok(h) if h.handle == st.handle => {
                    let key = crypto::key32(&unb64(&st.key)?)?;
                    return Ok(Some(UsernameLink {
                        link: format_link(&s.account.server_url, &key, &st.handle)?,
                        username,
                    }));
                }
                Ok(_) => {}
                Err(e) if e.code == ErrorCode::NotFound => {}
                Err(e) => return Err(e),
            }
        }
        self.create_username_link(username).await.map(Some)
    }

    /// Replaces the username link with a new one: links shared before stop working.
    pub async fn reset_username_link(&self) -> Result<Option<UsernameLink>> {
        let s = self.session()?;
        let dto: AccountDto = s.api.get("/v1/account").await?;
        match dto.username {
            Some(u) => self.create_username_link(u).await.map(Some),
            None => Ok(None),
        }
    }

    async fn create_username_link(&self, username: String) -> Result<UsernameLink> {
        let s = self.session()?;
        let key = crypto::random_bytes::<32>();
        let blob = encrypt_username(&key, &username)?;
        let h: HandleDto = s
            .api
            .put("/v1/account/username/link", &json!({ "encrypted_username": b64(&blob) }))
            .await?;
        self.db.borrow().kv_set_json(
            KV_KEY,
            &StoredLink { handle: h.handle.clone(), key: b64(key), username: username.clone() },
        )?;
        Ok(UsernameLink { link: format_link(&s.account.server_url, &key, &h.handle)?, username })
    }

    /// Opens a username link: returns the account it leads to (without adding it).
    pub async fn resolve_username_link(&self, link: &str) -> Result<FoundAccount> {
        let p = parse_link(link)?;
        let s = self.session()?;
        if let Some(server) = &p.server {
            if normalize_server_url(server).as_deref() != normalize_server_url(&s.account.server_url).as_deref() {
                return Err(CoreError::new(
                    ErrorCode::DifferentServer,
                    format!("this contact uses another server ({server})"),
                ));
            }
        }
        let blob: BlobDto = s
            .api
            .get(&format!("/v1/username/link/{}", p.handle))
            .await
            .map_err(|e| if e.code == ErrorCode::NotFound { CoreError::not_found("this username link is no longer valid") } else { e })?;
        let username = decrypt_username(&p.key, &unb64(&blob.encrypted_username)?)?;
        self.find_account(&username)
            .await?
            .ok_or_else(|| CoreError::not_found("this username link is no longer valid"))
    }

    /// Opens a username link and adds the account as a contact.
    pub async fn add_contact_from_username_link(self: &Rc<Self>, link: &str) -> Result<Contact> {
        let found = self.resolve_username_link(link).await?;
        match found.username {
            Some(u) => self.add_contact_by_query(&u, None).await,
            None => Err(CoreError::not_found("this username link is no longer valid")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_roundtrip() {
        let key = crypto::random_bytes::<32>();
        let handle = uuid::Uuid::new_v4().hyphenated().to_string();
        let link = format_link("https://chat.example.com:8443", &key, &handle).unwrap();
        assert!(is_username_link(&link));
        assert!(!link.contains("n="));
        let p = parse_link(&link).unwrap();
        assert_eq!(p.key, key);
        assert_eq!(p.handle, handle);
        assert_eq!(p.server.as_deref(), Some("https://chat.example.com:8443"));
        assert!(!is_username_link("chatapp://add?n=1234567812345678&s=x"));
        assert!(parse_link("chatapp://u#AAAA").is_err());
    }

    #[test]
    fn blob_roundtrip_hides_length() {
        let key = crypto::random_bytes::<32>();
        let a = encrypt_username(&key, "bob.01").unwrap();
        let b = encrypt_username(&key, "a_much_longer_name.123456").unwrap();
        assert_eq!(a.len(), b.len());
        assert_eq!(decrypt_username(&key, &a).unwrap(), "bob.01");
        let other = crypto::random_bytes::<32>();
        assert!(decrypt_username(&other, &a).is_err());
        assert!(encrypt_username(&key, &"x".repeat(33)).is_err());
    }
}
