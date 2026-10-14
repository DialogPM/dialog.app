//! The local database locked with the account password.
//!
//! The SQLCipher database is encrypted with a random 32-byte key. That key is stored next to
//! the database (`chat.db.lock`) wrapped with a key derived from the account password by
//! Argon2id, using a random salt that never leaves this device and its own domain prefix
//! (see [`crypto::derive_local_db_key`]). Without the password, the files on disk are
//! unreadable. Nothing relies on the OS keystore: before the data is locked (before login, or
//! right after linking a device, until the password is entered) it lives only in memory and
//! is written to disk, already encrypted, when it is locked.
//!
//! While locked, the engine runs on an empty in-memory database until [`Inner::unlock`].
//! Changing the password only re-wraps the key, it never re-encrypts the database.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use serde::{Deserialize, Serialize};

use super::Inner;
use crate::crypto::{self, b64, unb64, KdfParams};
use crate::db::Db;
use crate::error::{Context, CoreError, ErrorCode, Result};

pub const LOCK_FILE: &str = "chat.db.lock";
const LOCK_TMP: &str = "chat.db.lock.tmp";
const LOCK_AAD: &[u8] = b"chatapp/local-db/v1";

#[derive(Serialize, Deserialize)]
struct LockFile {
    v: u32,
    kdf: KdfParams,
    /// The database key, sealed with the password-derived key.
    wrapped_key: String,
}

async fn derive(password: &str, kdf: &KdfParams) -> Result<[u8; 32]> {
    let (password, kdf) = (password.to_string(), kdf.clone());
    tokio::task::spawn_blocking(move || crypto::derive_local_db_key(&password, &kdf))
        .await
        .ctx("local key derivation task")?
}

/// Makes an interrupted [`Inner::lock_with_password`] consistent again (startup only):
/// the new lock file is used if the database no longer opens with the device key.
pub fn finish_interrupted_lock(data_dir: &Path, device_key: &[u8; 32]) {
    let (lock, tmp) = (data_dir.join(LOCK_FILE), data_dir.join(LOCK_TMP));
    if !tmp.exists() {
        return;
    }
    if !lock.exists() && Db::open(&data_dir.join("chat.db"), device_key).is_err() {
        let _ = std::fs::rename(&tmp, &lock);
    } else {
        let _ = std::fs::remove_file(&tmp);
    }
}

pub fn is_locked_on_disk(data_dir: &Path) -> bool {
    data_dir.join(LOCK_FILE).exists()
}

impl Inner {
    fn lock_path(&self) -> PathBuf {
        self.data_dir.join(LOCK_FILE)
    }

    pub fn local_protected(&self) -> bool {
        self.lock_path().exists()
    }

    fn read_lock_file(&self) -> Result<LockFile> {
        let raw = std::fs::read(self.lock_path())?;
        Ok(serde_json::from_slice(&raw)?)
    }

    /// Writes the lock file for `key` atomically (to `path`, or the real lock file).
    async fn write_lock_file(&self, password: &str, key: &[u8; 32], path: &PathBuf) -> Result<()> {
        let kdf = KdfParams::new_random();
        let kek = derive(password, &kdf).await?;
        let file = LockFile { v: 1, kdf, wrapped_key: b64(crypto::aead_seal(&kek, key, LOCK_AAD)?) };
        let part = path.with_extension("part");
        std::fs::write(&part, serde_json::to_vec(&file)?)?;
        std::fs::rename(&part, path)?;
        Ok(())
    }

    /// Opens the real database with the password (startup screen).
    pub async fn unlock(self: &Rc<Self>, password: &str) -> Result<()> {
        if !self.locked.get() {
            return Ok(());
        }
        let file = self.read_lock_file()?;
        let kek = derive(password, &file.kdf).await?;
        let key = crypto::aead_open(&kek, &unb64(&file.wrapped_key)?, LOCK_AAD)
            .map_err(|_| CoreError::new(ErrorCode::InvalidPassword, "wrong password"))?;
        let key = crypto::key32(&key)?;
        if !self.locked.get() {
            // Unlocked meanwhile by a concurrent call.
            return Ok(());
        }
        let db = Db::open(&self.data_dir.join("chat.db"), &key)?;
        *self.db.borrow_mut() = db;
        self.db_key.set(key);
        self.locked.set(false);
        self.load_session()?;
        self.start_temporary_mode()?;
        self.note_password_strength(password);
        if self.session().is_ok() {
            self.clone().start_background();
        }
        Ok(())
    }

    /// Opens the real database with its key, already unwrapped with the password earlier in
    /// this run of the app (quick account switch, crate::profiles). Wrong key: invalid_password.
    pub async fn unlock_with_key(self: &Rc<Self>, key: [u8; 32]) -> Result<()> {
        if !self.locked.get() {
            return Ok(());
        }
        let db = Db::open(&self.data_dir.join("chat.db"), &key)
            .map_err(|_| CoreError::new(ErrorCode::InvalidPassword, "the saved key no longer opens this account"))?;
        *self.db.borrow_mut() = db;
        self.db_key.set(key);
        self.locked.set(false);
        self.load_session()?;
        self.start_temporary_mode()?;
        if self.session().is_ok() {
            self.clone().start_background();
        }
        Ok(())
    }

    /// The key of the database while it is open and locked with the password on disk (None
    /// otherwise): kept in memory by crate::profiles for quick switching back to this account.
    pub fn open_data_key(&self) -> Option<[u8; 32]> {
        (!self.locked.get() && self.local_protected()).then(|| self.db_key.get())
    }

    /// Re-encrypts the database with a fresh random key and locks that key with `password`.
    /// The key in the OS keystore no longer opens anything afterwards.
    pub async fn lock_with_password(&self, password: &str) -> Result<()> {
        if self.locked.get() {
            return Err(CoreError::invalid("the local data is locked"));
        }
        let key = crypto::random_bytes::<32>();
        let tmp = self.data_dir.join(LOCK_TMP);
        // Lock file first: if the app dies after the export/rekey, startup still finds the key.
        self.write_lock_file(password, &key, &tmp).await?;
        let db_path = self.data_dir.join("chat.db");
        let in_memory = self.db.borrow().is_memory();
        let result = if in_memory {
            // First write of the data to disk, encrypted with the new key from the start.
            for f in ["chat.db", "chat.db-wal", "chat.db-shm"] {
                let _ = std::fs::remove_file(self.data_dir.join(f));
            }
            let exported = self.db.borrow().export_to(&db_path, &key);
            match exported.and_then(|_| Db::open(&db_path, &key)) {
                Ok(db) => {
                    *self.db.borrow_mut() = db;
                    Ok(())
                }
                Err(e) => Err(e),
            }
        } else {
            // Older unprotected database on disk: re-encrypt it in place.
            self.db.borrow().rekey(&key)
        };
        if let Err(e) = result {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
        self.db_key.set(key);
        std::fs::rename(&tmp, self.lock_path())?;
        Ok(())
    }

    /// Locks the local data with the account password after checking it (data from older
    /// versions, linked devices, or a password changed on another device).
    pub async fn protect_local_data(&self, password: &str) -> Result<()> {
        self.check_account_password(password).await?;
        self.note_password_strength(password);
        if self.local_protected() {
            let key = self.db_key.get();
            self.write_lock_file(password, &key, &self.lock_path()).await
        } else {
            self.lock_with_password(password).await
        }
    }

    /// The password changed (on this device): re-wrap the database key.
    pub(super) async fn rewrap_local_key(&self, new_password: &str) -> Result<()> {
        if !self.local_protected() || self.locked.get() {
            return Ok(());
        }
        let key = self.db_key.get();
        self.write_lock_file(new_password, &key, &self.lock_path()).await
    }

    /// Checks the account password: locally first, then against the current account key on
    /// the server (when it was changed on another device; the local copy is updated then).
    pub async fn check_account_password(&self, password: &str) -> Result<()> {
        if matches!(self.verify_password(password).await, Ok(true)) {
            return Ok(());
        }
        let s = self.session()?;
        let dto: crate::api::AccountDto = s.api.get("/v1/account").await?;
        let (Some(kdf), Some(wrapped)) = (dto.kdf, dto.wrapped_account_key) else {
            return Err(CoreError::new(ErrorCode::InvalidPassword, "wrong password"));
        };
        let wrapped = unb64(&wrapped)?;
        let keys = {
            let (password, kdf) = (password.to_string(), kdf.clone());
            tokio::task::spawn_blocking(move || crypto::derive_password_keys(&password, &kdf))
                .await
                .ctx("password derivation task")??
        };
        let secret = crypto::unwrap_account_secret(&keys.wrap_key, &wrapped)
            .map_err(|_| CoreError::new(ErrorCode::InvalidPassword, "wrong password"))?;
        if !crypto::ct_eq(&secret.0, &self.account_secret()?.0) {
            return Err(CoreError::new(ErrorCode::InvalidPassword, "wrong password"));
        }
        let db = self.db.borrow();
        db.kv_set_json("kdf", &kdf)?;
        db.kv_set("wrapped_account_key", &wrapped)?;
        Ok(())
    }

    /// Whether the account password on the server differs from the one this device knows.
    pub(super) fn password_changed_elsewhere(&self, server_kdf: Option<&KdfParams>) -> bool {
        let Some(server) = server_kdf else { return false };
        match self.db.borrow().kv_get_json::<KdfParams>("kdf") {
            Ok(Some(local)) => local.salt != server.salt,
            _ => false,
        }
    }

    /// Forgotten password: deletes everything on this device so the account can be
    /// recovered (or logged in again) from scratch. History comes back from the backup.
    pub fn reset_local_data(&self) -> Result<()> {
        self.wipe_local("local_reset")
    }

    /// Removes the lock (after the database files were deleted).
    pub(super) fn remove_lock_files(&self) {
        for f in [LOCK_FILE, LOCK_TMP] {
            let _ = std::fs::remove_file(self.data_dir.join(f));
        }
        self.db_key.set(self.device_key);
        self.locked.set(false);
    }
}
