//! Profile key rotation.
//!
//! Our profile key decrypts our profile (name, about, avatar reference) and derives our
//! unidentified access key, which the server asks for before sealed-sender delivery,
//! anonymous pre-key fetches and presence subscriptions (server-rs ws/mod.rs). Everyone we
//! shared it with keeps those abilities, so blocking someone rotates it (as Signal does):
//!
//! 1. a fresh random key is chosen and kept as "next" (so a retry reuses it);
//! 2. the avatar is re-uploaded under a new attachment key and the profile is published
//!    encrypted with the next key (the server's copy under the old key is replaced);
//! 3. the next key becomes current: the new access key is uploaded (the server then drops
//!    presence subscriptions made with the old one), our other devices get it (sync message
//!    and vault settings), and it is sent again to everyone we share an accepted chat or
//!    group with, except blocked accounts.
//!
//! Group messages carry our profile key too; while a blocked account is a member of a group,
//! they go without it (engine/send.rs), so the key reaches the other members pairwise only.
//!
//! Before the first rotation the key is derived from the account secret (crypto.rs).

use serde_json::json;

use super::send::Job;
use super::Inner;
use crate::content::{Content, SyncMessage};
use crate::crypto::{self, b64, unb64};
use crate::error::{CoreError, Result};

const KEY_CURRENT: &str = "profile_key_rotated";
const KEY_NEXT: &str = "profile_key_next";

impl Inner {
    /// Our current profile key.
    pub fn profile_key(&self) -> Result<[u8; 32]> {
        if let Some(k) = self.db.borrow().kv_get(KEY_CURRENT)? {
            return crypto::key32(&k);
        }
        Ok(self.session()?.keys.profile_key)
    }

    /// Base64 of our current profile key (what contacts receive).
    pub fn profile_key_b64(&self) -> Result<String> {
        Ok(b64(self.profile_key()?))
    }

    /// Requests a rotation (e.g. after blocking someone); it runs from the outbox, so it is
    /// retried while offline.
    pub fn request_profile_key_rotation(&self) -> Result<()> {
        if self.db.borrow().kv_get(KEY_NEXT)?.is_none() {
            self.db.borrow().kv_set(KEY_NEXT, &crypto::random_bytes::<32>())?;
        }
        self.enqueue(&Job::RotateProfileKey)
    }

    /// Runs a pending rotation (outbox job). Nothing to do when none is pending.
    pub(super) async fn rotate_profile_key(&self) -> Result<()> {
        let Some(next) = self.db.borrow().kv_get(KEY_NEXT)? else { return Ok(()) };
        let next = crypto::key32(&next)?;
        let s = self.session()?;
        // The old avatar reference (attachment id and key) is known to everyone who had the
        // old key: upload the picture again under a new one.
        let mut profile = self.get_profile()?;
        if let Some(path) = profile.avatar_path.clone().filter(|p| std::path::Path::new(p).exists()) {
            if self.db.borrow().kv_get("profile_avatar")?.is_some() {
                let (local, avatar) = self.upload_avatar_from_path(&path, "self").await?;
                profile.avatar_path = Some(local);
                self.db.borrow().kv_set_json("profile_avatar", &avatar)?;
                self.db.borrow().kv_set_json("profile", &profile)?;
            }
        }
        self.publish_profile_with(&next).await?;
        {
            let db = self.db.borrow();
            db.kv_set(KEY_CURRENT, &next)?;
            db.kv_delete(KEY_NEXT)?;
        }
        let access_key = b64(crypto::unidentified_access_key(&next));
        match s.api.put::<_, serde_json::Value>("/v1/account/unidentified-access", &json!({ "key": access_key })).await {
            Ok(_) => self.db.borrow().kv_set_json("unidentified_access_uploaded", &access_key)?,
            // Uploaded again at the next connection (sealed.rs compares it).
            Err(e) => log::warn!("uploading the new access key: {e}"),
        }
        self.vault_put_settings().await;
        self.enqueue(&Job::OwnDevices { content: Content::Sync(SyncMessage::ProfileKey { key: b64(next) }) })?;
        self.announce_profile()?;
        self.emit("profile_updated", &self.get_profile()?);
        log::info!("profile key rotated");
        Ok(())
    }

    /// Another of our devices rotated the profile key.
    pub(super) async fn apply_profile_key_sync(&self, key_b64: &str) -> Result<()> {
        let key = unb64(key_b64)?;
        if key.len() != 32 {
            return Err(CoreError::invalid("invalid profile key"));
        }
        self.set_rotated_profile_key(&key)?;
        let me = self.me()?;
        self.fetch_profile(&me).await
    }

    /// Stores a profile key rotated elsewhere (sync message or vault settings).
    pub(super) fn set_rotated_profile_key(&self, key: &[u8]) -> Result<()> {
        let key = crypto::key32(key)?;
        if self.profile_key()? == key {
            return Ok(());
        }
        let db = self.db.borrow();
        db.kv_set(KEY_CURRENT, &key)?;
        // A rotation of our own still pending here would publish under another key.
        db.kv_delete(KEY_NEXT)?;
        Ok(())
    }

    /// The rotated key for the vault settings record (None: still the derived one).
    pub(super) fn rotated_profile_key_b64(&self) -> Option<String> {
        self.db.borrow().kv_get(KEY_CURRENT).ok().flatten().map(b64)
    }

    /// After a restart: a rotation whose outbox job gave up (offline for long) is retried.
    pub(super) fn resume_profile_key_rotation(&self) {
        if matches!(self.db.borrow().kv_get(KEY_NEXT), Ok(Some(_))) {
            if let Err(e) = self.enqueue(&Job::RotateProfileKey) {
                log::warn!("profile key rotation: {e}");
            }
        }
    }

    /// Whether a group has a member we blocked (its messages then go without our profile key).
    pub fn group_has_blocked_member(&self, group_id: &str) -> bool {
        self.group_member_ids(group_id).map(|ids| ids.iter().any(|a| self.is_blocked(a))).unwrap_or(false)
    }
}
