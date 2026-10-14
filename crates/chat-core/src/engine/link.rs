//! Linking a new device by QR code (docs/API.md, section 13).
//!
//! The QR code carries a session id and a random 32-byte secret. The account secret is sent
//! through the server encrypted with a key derived from that secret, so the server never
//! learns it. Either side may show the code:
//!
//! * new device shows, existing device scans: `link_create` + `link_wait` on the new device,
//!   `link_inspect` + `link_approve(Some(link))` on the existing one;
//! * existing device shows, new device scans: `link_offer` + `link_offer_wait` +
//!   `link_approve(None)` on the existing device, `link_claim` + `link_wait` on the new one.

use std::rc::Rc;
use std::time::Duration;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use hkdf::Hkdf;
use rand::Rng;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::Sha256;

use super::Inner;
use crate::api::ApiClient;
use crate::crypto::{self, b64, unb64, AccountKeys, AccountSecret, KdfParams};
use crate::error::{CoreError, ErrorCode, Result};
use crate::model::AccountInfo;
use crate::util::normalize_server_url;

const LINK_AAD: &[u8] = b"chatapp-link-v1";
const POLL_INTERVAL: Duration = Duration::from_millis(1500);

/// State of an in-progress link on this device.
#[derive(Clone)]
pub struct LinkSession {
    server_url: String,
    id: String,
    poll_key: String,
    secret: [u8; 32],
    /// True on the device that is being added.
    is_new_device: bool,
    device_name: String,
    /// Verification code both screens show (code shown by the existing device only).
    sas: Option<String>,
}

/// Separates the claimant's name from its verification nonce in the relayed device name.
const SAS_SEP: char = '\u{241F}';

/// 6-digit code from the QR secret and the claimant's nonce. Whoever photographs the QR code
/// can claim it first, but cannot make the user's own new device show the same code (and the
/// server, which never sees the secret, cannot forge one either).
fn sas_code(secret: &[u8; 32], nonce: &[u8]) -> String {
    let hk = Hkdf::<Sha256>::new(Some(nonce), secret);
    let mut out = [0u8; 4];
    hk.expand(b"chatapp-link-sas-v1", &mut out).expect("hkdf length");
    let n = u32::from_be_bytes(out) % 1_000_000;
    format!("{:03} {:03}", n / 1000, n % 1000)
}

/// Splits a relayed device name into the name and the claimant's nonce.
fn split_claimed_name(raw: &str) -> (String, Option<Vec<u8>>) {
    match raw.split_once(SAS_SEP) {
        Some((name, nonce)) => (name.to_string(), URL_SAFE_NO_PAD.decode(nonce).ok()),
        None => (raw.to_string(), None),
    }
}

struct ParsedLink {
    id: String,
    secret: [u8; 32],
    server_url: String,
}

#[derive(Deserialize)]
struct SessionView {
    state: String,
    device_name: Option<String>,
    #[serde(default)]
    payload: Option<String>,
    #[serde(default)]
    link_token: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct LinkPayload {
    account_secret: String,
    account_id: String,
    number: String,
    server_url: String,
}

#[derive(Deserialize)]
struct Created {
    id: String,
    poll_key: String,
}

#[derive(Deserialize)]
struct LinkResp {
    account_id: String,
    number: String,
    device_id: u32,
    token: String,
    wrapped_account_key: String,
    identity_key: String,
    kdf: KdfParams,
}

fn link_key(secret: &[u8; 32]) -> [u8; 32] {
    let hk = Hkdf::<Sha256>::new(None, secret);
    let mut out = [0u8; 32];
    hk.expand(b"chatapp-link-v1", &mut out).expect("valid length");
    out
}

fn format_link(id: &str, secret: &[u8; 32], server_url: &str) -> String {
    let server: String = url::form_urlencoded::byte_serialize(server_url.as_bytes()).collect();
    format!("chatapp://link?id={id}&k={}&s={server}", URL_SAFE_NO_PAD.encode(secret))
}

fn parse_link(link: &str) -> Result<ParsedLink> {
    let bad = || CoreError::invalid("this is not a device link code");
    let url = url::Url::parse(link.trim()).map_err(|_| bad())?;
    if url.scheme() != "chatapp" || url.host_str() != Some("link") {
        return Err(bad());
    }
    let (mut id, mut k, mut s) = (None, None, None);
    for (key, v) in url.query_pairs() {
        match key.as_ref() {
            "id" => id = Some(v.to_string()),
            "k" => k = Some(v.to_string()),
            "s" => s = Some(v.to_string()),
            _ => {}
        }
    }
    let id = id.filter(|i| i.len() == 32 && i.chars().all(|c| c.is_ascii_hexdigit())).ok_or_else(bad)?;
    let secret: [u8; 32] = URL_SAFE_NO_PAD
        .decode(k.ok_or_else(bad)?)
        .map_err(|_| bad())?
        .try_into()
        .map_err(|_| bad())?;
    let server_url = normalize_server_url(&s.ok_or_else(bad)?).ok_or_else(bad)?;
    Ok(ParsedLink { id, secret, server_url })
}

impl Inner {
    fn set_link(&self, s: Option<LinkSession>) {
        *self.link.borrow_mut() = s;
    }

    fn current_link(&self) -> Result<LinkSession> {
        self.link.borrow().clone().ok_or_else(|| CoreError::invalid("no device link in progress"))
    }

    async fn poll_link(&self, l: &LinkSession) -> Result<SessionView> {
        let api = ApiClient::new(&l.server_url, None);
        api.get(&format!("/v1/provisioning/{}/poll?key={}", l.id, l.poll_key)).await.map_err(|e| {
            if e.code == ErrorCode::NotFound {
                CoreError::new(ErrorCode::Gone, "the code expired, generate a new one")
            } else {
                e
            }
        })
    }

    // --- new device -------------------------------------------------------------------

    /// New device: creates a link session and returns the QR payload to show.
    pub async fn link_create(&self, server_url: &str, device_name: &str) -> Result<String> {
        if self.session().is_ok() {
            return Err(CoreError::invalid("already logged in"));
        }
        let url = normalize_server_url(server_url).ok_or_else(|| CoreError::invalid("invalid server URL"))?;
        let name = device_name.trim();
        let name = if name.is_empty() { "Dialog" } else { name };
        let created: Created = ApiClient::new(&url, None)
            .post("/v1/provisioning", &json!({ "mode": "new", "device_name": name }))
            .await?;
        let secret = crypto::random_bytes::<32>();
        let link = format_link(&created.id, &secret, &url);
        self.set_link(Some(LinkSession {
            server_url: url,
            id: created.id,
            poll_key: created.poll_key,
            secret,
            is_new_device: true,
            device_name: name.to_string(),
            sas: None,
        }));
        Ok(link)
    }

    /// New device: claims a code shown by an existing device.
    pub async fn link_claim(&self, link: &str, device_name: &str) -> Result<()> {
        if self.session().is_ok() {
            return Err(CoreError::invalid("already logged in"));
        }
        let p = parse_link(link)?;
        let name = device_name.trim();
        let name = if name.is_empty() { "Dialog" } else { name };
        #[derive(Deserialize)]
        struct Claimed {
            poll_key: String,
        }
        let nonce = crypto::random_bytes::<12>();
        let relayed = format!("{name}{SAS_SEP}{}", URL_SAFE_NO_PAD.encode(nonce));
        let claimed: Claimed = ApiClient::new(&p.server_url, None)
            .post(&format!("/v1/provisioning/{}/claim", p.id), &json!({ "device_name": relayed }))
            .await
            .map_err(|e| {
                if e.code == ErrorCode::NotFound || e.code == ErrorCode::Conflict {
                    CoreError::new(ErrorCode::Gone, "the code expired or was already used")
                } else {
                    e
                }
            })?;
        self.set_link(Some(LinkSession {
            server_url: p.server_url,
            id: p.id,
            poll_key: claimed.poll_key,
            secret: p.secret,
            is_new_device: true,
            device_name: name.to_string(),
            sas: Some(sas_code(&p.secret, &nonce)),
        }));
        Ok(())
    }

    /// New device: waits for the approval and logs in. Returns when linked.
    pub async fn link_wait(self: &Rc<Self>) -> Result<AccountInfo> {
        let l = self.current_link()?;
        if !l.is_new_device {
            return Err(CoreError::invalid("this device is not being linked"));
        }
        let view = loop {
            // Stop when the session was cancelled or replaced.
            match self.link.borrow().as_ref() {
                Some(cur) if cur.id == l.id => {}
                _ => return Err(CoreError::new(ErrorCode::Gone, "cancelled")),
            }
            let v = self.poll_link(&l).await?;
            if v.payload.is_some() {
                break v;
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        };
        self.set_link(None);
        let payload = unb64(view.payload.as_deref().unwrap_or_default())?;
        let plain = crypto::aead_open(&link_key(&l.secret), &payload, LINK_AAD)
            .map_err(|_| CoreError::internal("could not decrypt the account data"))?;
        let data: LinkPayload = serde_json::from_slice(&plain)?;
        let secret = AccountSecret(crypto::key32(&unb64(&data.account_secret)?)?);
        let keys = AccountKeys::derive(&secret)?;
        let registration_id = rand::rng().random_range(1..16380);
        let resp: LinkResp = ApiClient::new(&l.server_url, None)
            .post(
                "/v1/auth/link",
                &json!({
                    "link_token": view.link_token.unwrap_or_default(),
                    // Placeholder name: the real one is uploaded encrypted after login.
                    "device": super::security::placeholder_device(&self.platform, registration_id),
                }),
            )
            .await?;
        // The server must hand us the account that the other device actually sent.
        if resp.account_id != data.account_id
            || unb64(&resp.identity_key)? != keys.identity.identity_key().serialize().to_vec()
        {
            return Err(CoreError::internal("server returned a different account"));
        }
        let account = AccountInfo {
            account_id: resp.account_id,
            number: resp.number,
            device_id: resp.device_id,
            server_url: l.server_url.clone(),
        };
        let wrapped = unb64(&resp.wrapped_account_key)?;
        self.remember_device_name(&l.device_name);
        self.finish_login(account.clone(), resp.token, &secret, registration_id, &resp.kdf, &wrapped, true).await?;
        Ok(account)
    }

    /// The verification code of the current link (after link_claim on the new device, after
    /// link_offer_wait on the existing one): both screens must show the same digits.
    pub fn link_code(&self) -> Option<String> {
        self.link.borrow().as_ref().and_then(|l| l.sas.clone())
    }

    pub fn link_cancel(&self) {
        self.set_link(None);
    }

    // --- existing device ------------------------------------------------------------------

    /// Existing device: creates a code for a new device to scan.
    pub async fn link_offer(&self) -> Result<String> {
        let s = self.session()?;
        let created: Created = s.api.post("/v1/provisioning", &json!({ "mode": "existing" })).await?;
        let secret = crypto::random_bytes::<32>();
        let link = format_link(&created.id, &secret, &s.account.server_url);
        self.set_link(Some(LinkSession {
            server_url: s.account.server_url.clone(),
            id: created.id,
            poll_key: created.poll_key,
            secret,
            is_new_device: false,
            device_name: String::new(),
            sas: None,
        }));
        Ok(link)
    }

    /// Existing device: waits until a new device scanned our code; returns its name.
    pub async fn link_offer_wait(&self) -> Result<String> {
        let l = self.current_link()?;
        loop {
            match self.link.borrow().as_ref() {
                Some(cur) if cur.id == l.id => {}
                _ => return Err(CoreError::new(ErrorCode::Gone, "cancelled")),
            }
            let v = self.poll_link(&l).await?;
            if v.state == "claimed" {
                let (name, nonce) = split_claimed_name(&v.device_name.unwrap_or_default());
                // Without a nonce there is no code to compare: refuse rather than approve blind.
                let nonce = nonce.filter(|n| n.len() >= 12).ok_or_else(|| {
                    CoreError::new(ErrorCode::Gone, "the device that scanned the code cannot show a verification code")
                })?;
                if let Some(cur) = self.link.borrow_mut().as_mut().filter(|c| c.id == l.id) {
                    cur.sas = Some(sas_code(&l.secret, &nonce));
                }
                return Ok(name);
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// Existing device: reads the name of the device that shows `link`.
    pub async fn link_inspect(&self, link: &str) -> Result<String> {
        let s = self.session()?;
        let p = parse_link(link)?;
        if normalize_server_url(&s.account.server_url).as_deref() != Some(p.server_url.as_str()) {
            return Err(CoreError::new(ErrorCode::DifferentServer, format!("this device uses another server ({})", p.server_url)));
        }
        let v: SessionView = s.api.get(&format!("/v1/provisioning/{}", p.id)).await.map_err(|e| {
            if e.code == ErrorCode::NotFound {
                CoreError::new(ErrorCode::Gone, "the code expired, generate a new one on the other device")
            } else {
                e
            }
        })?;
        if v.state != "waiting" {
            return Err(CoreError::new(ErrorCode::Gone, "this code was already used"));
        }
        Ok(split_claimed_name(&v.device_name.unwrap_or_default()).0)
    }

    /// Existing device: sends the (encrypted) account to the new device.
    /// `link` is the scanned code; `None` approves the device that claimed our own code.
    pub async fn link_approve(&self, link: Option<&str>) -> Result<()> {
        let s = self.session()?;
        let (id, secret) = match link {
            Some(l) => {
                let p = parse_link(l)?;
                (p.id, p.secret)
            }
            None => {
                let l = self.current_link()?;
                (l.id, l.secret)
            }
        };
        let payload = LinkPayload {
            account_secret: b64(self.account_secret()?.0),
            account_id: s.account.account_id.clone(),
            number: s.account.number.clone(),
            server_url: s.account.server_url.clone(),
        };
        let sealed = crypto::aead_seal(&link_key(&secret), &serde_json::to_vec(&payload)?, LINK_AAD)?;
        let _: serde_json::Value = s
            .api
            .post(&format!("/v1/provisioning/{id}/payload"), &json!({ "payload": b64(sealed) }))
            .await?;
        if link.is_none() {
            self.set_link(None);
        }
        Ok(())
    }

    /// Checks the account password locally (app lock, sensitive actions).
    pub async fn verify_password(&self, password: &str) -> Result<bool> {
        match self.reveal_mnemonic(password).await {
            Ok(_) => Ok(true),
            Err(e) if e.code == ErrorCode::InvalidPassword => Ok(false),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod sas_tests {
    use super::*;

    #[test]
    fn codes() {
        let secret = [7u8; 32];
        let a = sas_code(&secret, b"nonce-one-12");
        assert_eq!(a.len(), 7);
        assert_eq!(a, sas_code(&secret, b"nonce-one-12"));
        assert_ne!(a, sas_code(&secret, b"nonce-two-12"));
        assert_ne!(a, sas_code(&[8u8; 32], b"nonce-one-12"));
        let (n, nonce) = split_claimed_name(&format!("Laptop{SAS_SEP}{}", URL_SAFE_NO_PAD.encode(b"abc")));
        assert_eq!((n.as_str(), nonce.as_deref()), ("Laptop", Some(&b"abc"[..])));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_roundtrip() {
        let secret = crypto::random_bytes::<32>();
        let link = format_link("0123456789abcdef0123456789abcdef", &secret, "https://chat.example.com");
        let p = parse_link(&link).unwrap();
        assert_eq!(p.secret, secret);
        assert_eq!(p.server_url, "https://chat.example.com");
        assert!(parse_link("chatapp://add?n=1").is_err());
    }
}
