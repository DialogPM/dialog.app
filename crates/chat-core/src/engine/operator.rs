//! Statements signed by the operator of the server: the badge of an official account and the
//! notices shown in the app.
//!
//! The server stores and delivers them, but the app does not take its word for either: each
//! carries an Ed25519 signature made in the operator's admin app, and it is checked against
//! the operator keys pinned from `/v1/info` at first contact (like the sealed sender trust
//! root, the zkgroup parameters and the key transparency key). A server that later announces
//! other keys does not get them adopted; a server that never announced any gets no badge and
//! no notice accepted.
//!
//! The signed strings are those of the server (server-rs/src/operator.rs) byte for byte:
//!
//! * badge:  `dialog-operator-badge-v1`, account id, base64(identity key), issued_at, label
//! * notice: `dialog-operator-notice-v1`, target (`*` or an account id), created_at,
//!   hex(sha256(title)), hex(sha256(body))
//!
//! A badge covers the identity key: it is checked against the key this device holds for the
//! account (the pinned Signal identity when there is one), so a badge does not pass to whoever
//! the server gives the account's key to. A notice covers its target: one for another account
//! is not shown to us. Both are derived data of this device: never put in the vault.

use ed25519_dalek::{Signature, VerifyingKey};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::Inner;
use crate::crypto::{b64, sha256, unb64};
use crate::error::Result;
use crate::model::Notice;
use crate::util::now_ms;

pub const BADGE_CONTEXT: &str = "dialog-operator-badge-v1";
pub const NOTICE_CONTEXT: &str = "dialog-operator-notice-v1";
/// Target of a notice for every account.
pub const EVERYONE: &str = "*";
pub const MAX_LABEL: usize = 40;
pub const MAX_TITLE: usize = 120;
pub const MAX_BODY: usize = 2000;
/// Notices kept on this device (the newest).
const KEEP_NOTICES: i64 = 200;

/// The operator keys pinned at first contact (base64, JSON array).
const KEYS_KEY: &str = "operator_keys";
/// The last set of keys the server announced that differs from the pinned one (to log it once).
const REFUSED_KEY: &str = "operator_keys_refused";
/// Id of the newest notice fetched from the server.
const CURSOR_KEY: &str = "notices_after";

/// The badge of an official account, as directory entries and profiles carry it.
#[derive(Debug, Clone, Deserialize)]
pub struct BadgeDto {
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub issued_at: i64,
    #[serde(default)]
    pub key: String,
    #[serde(default)]
    pub signature: String,
}

#[derive(Debug, Clone, Deserialize)]
struct NoticeDto {
    id: i64,
    #[serde(default)]
    target: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    created_at: i64,
    #[serde(default)]
    key: String,
    #[serde(default)]
    signature: String,
}

#[derive(Deserialize)]
struct NoticesPage {
    #[serde(default)]
    notices: Vec<NoticeDto>,
    #[serde(default)]
    more: bool,
}

/// The reply of `list_notices`.
#[derive(Debug, Clone, Serialize)]
pub struct NoticeList {
    pub notices: Vec<Notice>,
    pub unread: i64,
}

pub fn badge_string(account_id: &str, identity_key: &[u8], issued_at: i64, label: &str) -> String {
    format!("{BADGE_CONTEXT}\n{account_id}\n{}\n{issued_at}\n{label}", b64(identity_key))
}

pub fn notice_string(target: &str, created_at: i64, title: &str, body: &str) -> String {
    format!(
        "{NOTICE_CONTEXT}\n{target}\n{created_at}\n{}\n{}",
        hex::encode(sha256(title.as_bytes())),
        hex::encode(sha256(body.as_bytes()))
    )
}

/// Whether `signature` (base64) by `key` (base64), one of the pinned keys, verifies `message`.
fn signed_by_pinned(pinned: &[[u8; 32]], key: &str, signature: &str, message: &str) -> bool {
    let Some(key) = unb64(key).ok().and_then(|k| <[u8; 32]>::try_from(k).ok()) else { return false };
    if !pinned.contains(&key) {
        return false;
    }
    let (Ok(vk), Some(sig)) = (VerifyingKey::from_bytes(&key), unb64(signature).ok().and_then(|s| Signature::from_slice(&s).ok()))
    else {
        return false;
    };
    vk.verify_strict(message.as_bytes(), &sig).is_ok()
}

/// One line of visible text exactly as signed: no control characters, no spaces around it,
/// 1..=max characters.
fn is_clean_line(s: &str, max: usize) -> bool {
    let n = s.chars().count();
    n >= 1 && n <= max && s.trim() == s && !s.chars().any(char::is_control)
}

/// Text of several lines exactly as signed: no control characters but `\n`, 1..=max characters.
fn is_clean_text(s: &str, max: usize) -> bool {
    let n = s.chars().count();
    n >= 1 && n <= max && s.trim() == s && !s.chars().any(|c| c.is_control() && c != '\n')
}

/// The label of a valid badge of `account_id`, whose identity key (as this device knows it) is
/// `identity_key`. None for anything else.
pub fn verify_badge(pinned: &[[u8; 32]], account_id: &str, identity_key: &[u8], badge: &BadgeDto) -> Option<String> {
    if !is_clean_line(&badge.label, MAX_LABEL) {
        return None;
    }
    let message = badge_string(account_id, identity_key, badge.issued_at, &badge.label);
    signed_by_pinned(pinned, &badge.key, &badge.signature, &message).then(|| badge.label.clone())
}

/// Whether a notice the server delivered to `me` was signed by the operator for `me` (or for
/// everyone), with text within the limits.
fn verify_notice(pinned: &[[u8; 32]], me: &str, n: &NoticeDto) -> bool {
    let for_us = n.target == EVERYONE || super::sealed::same_account(&n.target, me);
    for_us
        && is_clean_line(&n.title, MAX_TITLE)
        && is_clean_text(&n.body, MAX_BODY)
        && signed_by_pinned(pinned, &n.key, &n.signature, &notice_string(&n.target, n.created_at, &n.title, &n.body))
}

/// The valid 32-byte Ed25519 keys of a list in base64, without duplicates.
fn parse_keys(keys: &[String]) -> Vec<[u8; 32]> {
    let mut out: Vec<[u8; 32]> = Vec::new();
    for k in keys {
        let Some(key) = unb64(k).ok().and_then(|k| <[u8; 32]>::try_from(k).ok()) else { continue };
        if VerifyingKey::from_bytes(&key).is_ok() && !out.contains(&key) {
            out.push(key);
        }
    }
    out
}

impl Inner {
    /// The pinned operator keys (empty: none pinned, nothing is accepted).
    pub(super) fn operator_keys(&self) -> Vec<[u8; 32]> {
        let keys: Vec<String> = self.db.borrow().kv_get_json(KEYS_KEY).ok().flatten().unwrap_or_default();
        parse_keys(&keys)
    }

    /// Pins the operator keys (each time the connection starts), then catches up on our own
    /// badge and the notices. The first non-empty set is pinned; a different set announced
    /// later is not adopted (and logged once).
    pub(super) async fn operator_setup(&self) -> Result<()> {
        let info = self.session()?.api.info().await?;
        let announced = parse_keys(&info.operator_keys);
        let pinned = self.operator_keys();
        if pinned.is_empty() {
            if !announced.is_empty() {
                let keys: Vec<String> = announced.iter().map(b64).collect();
                self.db.borrow().kv_set_json(KEYS_KEY, &keys)?;
            }
        } else {
            let same = announced.len() == pinned.len() && announced.iter().all(|k| pinned.contains(k));
            if !same {
                let keys: Vec<String> = announced.iter().map(b64).collect();
                let logged: Option<Vec<String>> = self.db.borrow().kv_get_json(REFUSED_KEY)?;
                if logged.as_ref() != Some(&keys) {
                    log::error!("the server announces other operator keys than the pinned ones: keeping the pinned keys");
                    self.db.borrow().kv_set_json(REFUSED_KEY, &keys)?;
                }
            }
        }
        if let Err(e) = self.refresh_own_badge().await {
            log::debug!("own badge: {e}");
        }
        self.fetch_notices().await
    }

    /// The identity key a badge of `account_id` must be signed over: ours for our account, the
    /// pinned Signal identity for anyone else (None when there is none yet).
    fn badge_identity(&self, account_id: &str) -> Option<Vec<u8>> {
        let s = self.session().ok()?;
        if super::sealed::same_account(account_id, &s.account.account_id) {
            return Some(s.keys.identity.identity_key().serialize().to_vec());
        }
        self.db
            .borrow()
            .conn
            .query_row("SELECT identity_key FROM identities WHERE account_id = ?1", [account_id], |r| r.get(0))
            .optional()
            .ok()
            .flatten()
    }

    /// What a fresh directory entry or profile says about `account_id`'s badge: `Some(label)`
    /// for a valid one, `Some(None)` for none (absent or not valid), and None when it cannot
    /// be judged because this device knows no identity key for the account
    /// (`entry_identity_key`: the key of the same directory entry, used only when this device
    /// has not pinned one).
    pub(super) fn judge_badge(&self, account_id: &str, entry_identity_key: Option<&str>, badge: Option<&BadgeDto>) -> Option<Option<String>> {
        let Some(badge) = badge else { return Some(None) };
        let pinned = self.operator_keys();
        if pinned.is_empty() {
            return Some(None);
        }
        let identity = self.badge_identity(account_id).or_else(|| entry_identity_key.and_then(|k| unb64(k).ok()))?;
        let label = verify_badge(&pinned, account_id, &identity, badge);
        if label.is_none() {
            log::warn!("a badge served for {account_id} does not verify: not shown");
        }
        Some(label)
    }

    /// `judge_badge` of a directory entry, which always carries the account's key.
    pub(super) fn entry_badge(&self, entry: &crate::api::DirectoryEntryDto) -> Option<String> {
        self.judge_badge(&entry.account_id, Some(&entry.identity_key), entry.badge.as_ref()).flatten()
    }

    /// Stores what a fresh directory entry or profile says about `account_id`'s badge (`label`:
    /// the verified label, None: no valid badge). Returns whether it changed; the caller
    /// announces the contact.
    pub(super) fn store_badge(&self, account_id: &str, label: Option<&str>) -> Result<bool> {
        let changed = self.db.borrow().conn.execute(
            "UPDATE contacts SET official = ?2 WHERE account_id = ?1 AND official IS NOT ?2",
            params![account_id, label],
        )?;
        Ok(changed > 0)
    }

    /// `store_badge`, announcing the contact (and its conversation) when it changed.
    pub(super) fn apply_badge(&self, account_id: &str, label: Option<&str>) -> Result<()> {
        if self.store_badge(account_id, label)? {
            self.emit_contact(account_id);
        }
        Ok(())
    }

    /// Our own badge, as the profile shows it. Announces the profile when it changed.
    pub(super) fn set_own_badge(&self, label: Option<String>) -> Result<()> {
        let mut p = self.get_profile()?;
        if p.official != label {
            p.official = label;
            self.db.borrow().kv_set_json("profile", &p)?;
            self.emit("profile_updated", &p);
        }
        Ok(())
    }

    /// Reads our own directory entry for our badge.
    async fn refresh_own_badge(&self) -> Result<()> {
        let s = self.session()?;
        let me = s.account.account_id.clone();
        let entry: crate::api::DirectoryEntryDto = s.api.get(&format!("/v1/directory/account/{me}")).await?;
        // Checked against our own identity key, whatever key the entry carries.
        let label = self.judge_badge(&me, None, entry.badge.as_ref()).flatten();
        self.set_own_badge(label)
    }

    // --- notices ----------------------------------------------------------------------

    /// Fetches the notices the server has for us after the last one fetched. Notices that do
    /// not verify are skipped (the cursor still moves past them: asking again gives the same
    /// bytes). Without pinned operator keys nothing is fetched, so nothing is skipped either.
    pub(super) async fn fetch_notices(&self) -> Result<()> {
        let pinned = self.operator_keys();
        if pinned.is_empty() {
            return Ok(());
        }
        let s = self.session()?;
        let me = s.account.account_id.clone();
        let mut added = 0;
        // Bounded: a server that always answers "more" cannot keep us here.
        for _ in 0..50 {
            let after: i64 = self.db.borrow().kv_get_json(CURSOR_KEY)?.unwrap_or(0);
            let page: NoticesPage = s.api.get(&format!("/v1/notices?after={after}")).await?;
            let mut last = after;
            for n in &page.notices {
                if n.id <= last {
                    // Out of order or repeated: the server must answer in id order.
                    continue;
                }
                last = n.id;
                if !verify_notice(&pinned, &me, n) {
                    log::warn!("notice {} does not verify against the pinned operator keys: skipped", n.id);
                    continue;
                }
                let inserted = self.db.borrow().conn.execute(
                    "INSERT OR IGNORE INTO notices (id, title, body, created_at, received_at, read) VALUES (?1, ?2, ?3, ?4, ?5, 0)",
                    params![n.id, n.title, n.body, n.created_at, now_ms()],
                )?;
                if inserted > 0 {
                    added += 1;
                    if let Some(notice) = self.notice(n.id)? {
                        self.emit("notice", &notice);
                    }
                }
            }
            if last > after {
                // Never back: a concurrent fetch may have gone further.
                let current: i64 = self.db.borrow().kv_get_json(CURSOR_KEY)?.unwrap_or(0);
                self.db.borrow().kv_set_json(CURSOR_KEY, &last.max(current))?;
            }
            if !page.more || last == after {
                break;
            }
        }
        if added > 0 {
            self.db.borrow().conn.execute(
                "DELETE FROM notices WHERE id NOT IN (SELECT id FROM notices ORDER BY id DESC LIMIT ?1)",
                [KEEP_NOTICES],
            )?;
            self.emit_notices_changed()?;
        }
        Ok(())
    }

    /// Fetches the notices in the background (a server event said there is a new one).
    pub(super) fn spawn_notices_fetch(self: &std::rc::Rc<Self>) {
        let me = self.clone();
        tokio::task::spawn_local(async move {
            if let Err(e) = me.fetch_notices().await {
                log::warn!("notices: {e}");
            }
        });
    }

    fn notice(&self, id: i64) -> Result<Option<Notice>> {
        Ok(self
            .db
            .borrow()
            .conn
            .query_row("SELECT id, title, body, created_at, received_at, read FROM notices WHERE id = ?1", [id], notice_from_row)
            .optional()?)
    }

    fn unread_notices(&self) -> Result<i64> {
        Ok(self.db.borrow().conn.query_row("SELECT COUNT(*) FROM notices WHERE read = 0", [], |r| r.get(0))?)
    }

    fn emit_notices_changed(&self) -> Result<()> {
        let unread = self.unread_notices()?;
        self.emit("notices_changed", &json!({ "unread": unread }));
        Ok(())
    }

    /// The notices on this device, newest first, and how many are unread.
    pub fn list_notices(&self) -> Result<NoticeList> {
        let notices = self
            .db
            .borrow()
            .conn
            .prepare("SELECT id, title, body, created_at, received_at, read FROM notices ORDER BY id DESC")?
            .query_map([], notice_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(NoticeList { notices, unread: self.unread_notices()? })
    }

    pub fn mark_notices_read(&self) -> Result<serde_json::Value> {
        let changed = self.db.borrow().conn.execute("UPDATE notices SET read = 1 WHERE read = 0", [])?;
        if changed > 0 {
            self.emit_notices_changed()?;
        }
        Ok(json!({ "unread": 0 }))
    }

    pub fn delete_notice(&self, id: i64) -> Result<serde_json::Value> {
        let unread_before = self.unread_notices()?;
        self.db.borrow().conn.execute("DELETE FROM notices WHERE id = ?1", [id])?;
        if self.unread_notices()? != unread_before {
            self.emit_notices_changed()?;
        }
        Ok(json!({}))
    }
}

fn notice_from_row(r: &rusqlite::Row) -> rusqlite::Result<Notice> {
    Ok(Notice {
        id: r.get(0)?,
        title: r.get(1)?,
        body: r.get(2)?,
        created_at: r.get(3)?,
        received_at: r.get(4)?,
        read: r.get::<_, i64>(5)? != 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    const ACCOUNT: &str = "0b6c8a52-3c1e-4f0e-9a43-2d4f7d1e8b11";
    const OTHER: &str = "1b6c8a52-3c1e-4f0e-9a43-2d4f7d1e8b11";
    const T: i64 = 1_790_000_000_000;

    fn signer() -> (SigningKey, [u8; 32]) {
        let sk = SigningKey::from_bytes(&[7u8; 32]);
        let pk = sk.verifying_key().to_bytes();
        (sk, pk)
    }

    fn badge(sk: &SigningKey, account: &str, identity: &[u8], issued_at: i64, label: &str) -> BadgeDto {
        let sig = sk.sign(badge_string(account, identity, issued_at, label).as_bytes()).to_bytes();
        BadgeDto { label: label.into(), issued_at, key: b64(sk.verifying_key().to_bytes()), signature: b64(sig) }
    }

    fn notice(sk: &SigningKey, target: &str, title: &str, body: &str) -> NoticeDto {
        let sig = sk.sign(notice_string(target, T, title, body).as_bytes()).to_bytes();
        NoticeDto {
            id: 1,
            target: target.into(),
            title: title.into(),
            body: body.into(),
            created_at: T,
            key: b64(sk.verifying_key().to_bytes()),
            signature: b64(sig),
        }
    }

    #[test]
    fn the_signed_strings_are_those_of_the_server() {
        assert_eq!(
            badge_string("0b6c8a52-3c1e-4f0e-9a43-2d4f7d1e8b11", &[5, 1, 2], 1790000000000, "Dialog Support"),
            "dialog-operator-badge-v1\n0b6c8a52-3c1e-4f0e-9a43-2d4f7d1e8b11\nBQEC\n1790000000000\nDialog Support"
        );
        assert_eq!(
            notice_string("*", 1790000000000, "Hi", "there"),
            "dialog-operator-notice-v1\n*\n1790000000000\n3639efcd08abb273b1619e82e78c29a7df02c1051b1820e99fc395dcaa3326b8\ne244f187f696561d5fd7e00f618e7ba641dc52e3c137380f6fa23a854b773aac"
        );
    }

    #[test]
    fn a_badge_verifies_only_for_what_it_was_signed_about() {
        let (sk, pk) = signer();
        let pinned = [pk];
        let identity = [5u8; 33];
        let good = badge(&sk, ACCOUNT, &identity, T, "Dialog Support");
        assert_eq!(verify_badge(&pinned, ACCOUNT, &identity, &good).as_deref(), Some("Dialog Support"));

        // Signed by a key that is not pinned.
        let other_signer = SigningKey::from_bytes(&[8u8; 32]);
        assert!(verify_badge(&pinned, ACCOUNT, &identity, &badge(&other_signer, ACCOUNT, &identity, T, "Dialog Support")).is_none());
        // Signed by a pinned key, but presented with another key's name.
        let mut renamed = good.clone();
        renamed.key = b64(other_signer.verifying_key().to_bytes());
        assert!(verify_badge(&[pk, other_signer.verifying_key().to_bytes()], ACCOUNT, &identity, &renamed).is_none());
        // Nothing pinned.
        assert!(verify_badge(&[], ACCOUNT, &identity, &good).is_none());
        // Copied to another account, or the account's identity key replaced.
        assert!(verify_badge(&pinned, OTHER, &identity, &good).is_none());
        assert!(verify_badge(&pinned, ACCOUNT, &[6u8; 33], &good).is_none());
        // Altered label or time.
        let mut b = good.clone();
        b.label = "Dialog Admin".into();
        assert!(verify_badge(&pinned, ACCOUNT, &identity, &b).is_none());
        let mut b = good.clone();
        b.issued_at += 1;
        assert!(verify_badge(&pinned, ACCOUNT, &identity, &b).is_none());
        // Malformed signature or key.
        for (key, sig) in [
            (good.key.clone(), good.signature[..40].to_string()),
            (good.key.clone(), "not base64!".to_string()),
            (good.key.clone(), String::new()),
            (b64([1u8; 31]), good.signature.clone()),
            ("%%%".to_string(), good.signature.clone()),
        ] {
            let b = BadgeDto { key, signature: sig, ..good.clone() };
            assert!(verify_badge(&pinned, ACCOUNT, &identity, &b).is_none());
        }
        // A label that is not one clean line, even when signed.
        for label in ["Dialog\nSupport", "", " Dialog", "Dialog\u{7}", &"x".repeat(MAX_LABEL + 1)] {
            assert!(verify_badge(&pinned, ACCOUNT, &identity, &badge(&sk, ACCOUNT, &identity, T, label)).is_none(), "{label:?}");
        }
        assert!(verify_badge(&pinned, ACCOUNT, &identity, &badge(&sk, ACCOUNT, &identity, T, &"x".repeat(MAX_LABEL))).is_some());
    }

    #[test]
    fn a_notice_verifies_only_for_its_target_and_text() {
        let (sk, pk) = signer();
        let pinned = [pk];
        let everyone = notice(&sk, EVERYONE, "Maintenance", "Tonight at 23:00.\nAbout an hour.");
        assert!(verify_notice(&pinned, ACCOUNT, &everyone));
        let ours = notice(&sk, ACCOUNT, "Hello", "Only for you.");
        assert!(verify_notice(&pinned, ACCOUNT, &ours));
        // The same id in capitals is the same account.
        assert!(verify_notice(&pinned, &ACCOUNT.to_uppercase(), &ours));

        // Another account's notice, served to us as is or relabeled as ours or as everyone's.
        let theirs = notice(&sk, OTHER, "Hello", "Only for you.");
        assert!(!verify_notice(&pinned, ACCOUNT, &theirs));
        assert!(!verify_notice(&pinned, ACCOUNT, &NoticeDto { target: ACCOUNT.into(), ..theirs.clone() }));
        assert!(!verify_notice(&pinned, ACCOUNT, &NoticeDto { target: EVERYONE.into(), ..theirs }));
        // Altered title, body or time.
        assert!(!verify_notice(&pinned, ACCOUNT, &NoticeDto { title: "Maintenance!".into(), ..everyone.clone() }));
        assert!(!verify_notice(&pinned, ACCOUNT, &NoticeDto { body: "Tonight at 22:00.\nAbout an hour.".into(), ..everyone.clone() }));
        assert!(!verify_notice(&pinned, ACCOUNT, &NoticeDto { created_at: T + 1, ..everyone.clone() }));
        // Signed by a key that is not pinned; nothing pinned.
        let other_signer = SigningKey::from_bytes(&[8u8; 32]);
        assert!(!verify_notice(&pinned, ACCOUNT, &notice(&other_signer, EVERYONE, "Maintenance", "Tonight.")));
        assert!(!verify_notice(&[], ACCOUNT, &everyone));
        // Malformed signature.
        assert!(!verify_notice(&pinned, ACCOUNT, &NoticeDto { signature: everyone.signature[..20].into(), ..everyone.clone() }));
        // Text over the limits or with control characters, even when signed.
        assert!(!verify_notice(&pinned, ACCOUNT, &notice(&sk, EVERYONE, &"t".repeat(MAX_TITLE + 1), "body")));
        assert!(!verify_notice(&pinned, ACCOUNT, &notice(&sk, EVERYONE, "two\nlines", "body")));
        assert!(!verify_notice(&pinned, ACCOUNT, &notice(&sk, EVERYONE, "title", &"b".repeat(MAX_BODY + 1))));
        assert!(!verify_notice(&pinned, ACCOUNT, &notice(&sk, EVERYONE, "title", "bell\u{7}")));
        assert!(verify_notice(&pinned, ACCOUNT, &notice(&sk, EVERYONE, &"t".repeat(MAX_TITLE), &"b".repeat(MAX_BODY))));
    }

    #[test]
    fn malformed_operator_keys_are_ignored() {
        let (_, pk) = signer();
        let keys = vec![b64(pk), b64(pk), b64([1u8; 31]), "nope".into(), String::new()];
        assert_eq!(parse_keys(&keys), vec![pk]);
    }
}
