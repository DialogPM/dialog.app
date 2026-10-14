//! Registration, login, recovery, keys and account settings.

use std::rc::Rc;

use libsignal_protocol::{
    kem, GenericSignedPreKey, KeyPair, KyberPreKeyId, KyberPreKeyRecord, PreKeyId, PreKeyRecord,
    SignedPreKeyId, SignedPreKeyRecord, Timestamp,
};
use rand::Rng;
use rusqlite::params;
use serde::Deserialize;
use serde_json::json;

use super::security::{self, PendingLogin};
use super::Inner;
use crate::api::*;
use crate::crypto::{self, b64, unb64, AccountKeys, AccountSecret, KdfParams};
use crate::error::{Context, CoreError, ErrorCode, Result};
use crate::model::*;
use crate::util::{normalize_number, normalize_server_url, now_ms};

const PREKEY_BATCH: u32 = 100;
const KYBER_BATCH: u32 = 50;
const PREKEY_LOW: i64 = 20;
const SIGNED_PREKEY_ROTATION_MS: i64 = 7 * 24 * 3600 * 1000;

async fn derive_password(password: String, kdf: KdfParams) -> Result<crypto::PasswordKeys> {
    tokio::task::spawn_blocking(move || crypto::derive_password_keys(&password, &kdf))
        .await
        .ctx("password derivation task")?
}

/// New passwords must meet the password rules (see util::password_problems).
fn validate_password(password: &str) -> Result<()> {
    let problems = crate::util::password_problems(password);
    if !problems.is_empty() {
        return Err(CoreError::new(ErrorCode::WeakPassword, format!("password too weak: {}", problems.join(", "))));
    }
    Ok(())
}

/// The device as sent to the server: a placeholder name (the real one is uploaded encrypted
/// for the account's own devices right after login, see security.rs).
fn device_dto(platform: &str, registration_id: u32) -> DeviceDto {
    security::placeholder_device(platform, registration_id)
}

fn new_registration_id() -> u32 {
    rand::rng().random_range(1..16380)
}

impl Inner {
    pub fn app_state(&self) -> AppState {
        let account = self.session().ok().map(|s| s.account.clone());
        AppState {
            status: if self.locked.get() {
                "locked"
            } else if account.is_some() {
                "ready"
            } else {
                "no_account"
            },
            account,
            default_server_url: self.default_server_url.clone(),
            pending_mnemonic: self.db.borrow().kv_get_json("pending_mnemonic").ok().flatten(),
            local_protected: self.local_protected(),
        }
    }

    /// Forgets the recovery phrase kept until the user confirmed having saved it.
    pub fn confirm_mnemonic_saved(&self) -> Result<()> {
        self.db.borrow().kv_delete("pending_mnemonic")
    }

    pub async fn check_server(&self, url: &str) -> Result<ServerInfo> {
        let url = normalize_server_url(url).ok_or_else(|| CoreError::invalid("invalid server URL"))?;
        let info = ApiClient::new(&url, None).info().await?;
        Ok(ServerInfo {
            url,
            name: info.name,
            version: info.version,
            registration_enabled: info.registration_enabled,
            phone_verification_enabled: info.phone_verification_enabled,
            max_attachment_bytes: info.max_attachment_bytes,
            max_devices: info.max_devices,
        })
    }

    fn ensure_logged_out(&self) -> Result<()> {
        if self.session().is_ok() {
            return Err(CoreError::invalid("already logged in"));
        }
        Ok(())
    }

    pub async fn register(
        self: &Rc<Self>,
        server_url: &str,
        password: &str,
        device_name: &str,
    ) -> Result<RegisterResult> {
        self.ensure_logged_out()?;
        validate_password(password)?;
        let url = normalize_server_url(server_url).ok_or_else(|| CoreError::invalid("invalid server URL"))?;
        let api = ApiClient::new(&url, None);
        let (words, secret) = crypto::generate_mnemonic();
        let keys = AccountKeys::derive(&secret)?;
        let kdf = KdfParams::new_random();
        let pw = derive_password(password.to_string(), kdf.clone()).await?;
        let registration_id = new_registration_id();
        let wrapped = crypto::wrap_account_secret(&pw.wrap_key, &secret)?;
        let pow = self.registration_pow(&api).await?;
        let req = RegisterReq {
            kdf: kdf.clone(),
            auth_key: b64(pw.auth_key),
            wrapped_account_key: b64(&wrapped),
            identity_key: b64(keys.identity.identity_key().serialize()),
            recovery_public_key: b64(keys.recovery_public_key()),
            device: device_dto(&self.platform, registration_id),
            pow,
        };
        let resp: RegisterResp = api.post("/v1/accounts", &req).await?;
        let account = AccountInfo {
            account_id: resp.account_id,
            number: resp.number,
            device_id: resp.device_id,
            server_url: url,
        };
        self.remember_device_name(device_name);
        self.finish_login(account.clone(), resp.token, &secret, registration_id, &kdf, &wrapped, false).await?;
        self.lock_after_login(password).await;
        // Kept (in the encrypted local database) so it can be shown again if the app is
        // closed before the user confirmed writing it down.
        self.db.borrow().kv_set_json("pending_mnemonic", &words)?;
        Ok(RegisterResult { account, mnemonic: words })
    }

    pub async fn login(
        self: &Rc<Self>,
        server_url: &str,
        number: &str,
        password: &str,
        device_name: &str,
        totp: Option<&str>,
    ) -> Result<AccountInfo> {
        self.ensure_logged_out()?;
        let url = normalize_server_url(server_url).ok_or_else(|| CoreError::invalid("invalid server URL"))?;
        let number = normalize_number(number).ok_or_else(|| CoreError::invalid("invalid account number"))?;
        let api = ApiClient::new(&url, None);
        #[derive(Deserialize)]
        struct Pre {
            kdf: KdfParams,
        }
        let pre: Pre = api.post("/v1/auth/prelogin", &json!({ "number": number })).await?;
        let pw = derive_password(password.to_string(), pre.kdf.clone()).await?;
        let registration_id = new_registration_id();
        let display_name = security::clean_device_name(device_name);
        let resp: serde_json::Value = api
            .post(
                "/v1/auth/login",
                &json!({
                    "number": number,
                    "auth_key": b64(pw.auth_key),
                    "device": device_dto(&self.platform, registration_id),
                    // Shown to our other devices if the login needs their approval.
                    "display_name": display_name,
                    "totp": totp.map(|c| c.trim()).unwrap_or(""),
                }),
            )
            .await
            .map_err(|e| {
                if e.code == ErrorCode::Unauthorized {
                    CoreError::new(ErrorCode::InvalidPassword, "wrong account number or password")
                } else {
                    e
                }
            })?;
        if resp.get("status").and_then(|v| v.as_str()) == Some("approval_required") {
            let field = |k: &str| resp.get(k).and_then(|v| v.as_str()).unwrap_or_default().to_string();
            return Err(self.hold_pending_login(PendingLogin {
                url,
                request_id: field("request_id"),
                secret: field("secret"),
                wrap_key: zeroize::Zeroizing::new(pw.wrap_key),
                kdf: pre.kdf.clone(),
                registration_id,
                password: zeroize::Zeroizing::new(password.to_string()),
                device_name: display_name,
            }));
        }
        let resp: LoginResp =
            serde_json::from_value(resp).map_err(|e| CoreError::internal(format!("decode login: {e}")))?;
        self.complete_password_login(&url, resp, &pw.wrap_key, &pre.kdf, registration_id, password, &display_name)
            .await
    }

    /// Finishes a password login accepted by the server (directly or after approval).
    pub(super) async fn complete_password_login(
        self: &Rc<Self>,
        url: &str,
        resp: LoginResp,
        wrap_key: &[u8; 32],
        kdf: &KdfParams,
        registration_id: u32,
        password: &str,
        device_name: &str,
    ) -> Result<AccountInfo> {
        let wrapped = unb64(&resp.wrapped_account_key)?;
        let secret = crypto::unwrap_account_secret(wrap_key, &wrapped)?;
        let keys = AccountKeys::derive(&secret)?;
        if let Some(ik) = &resp.identity_key {
            if unb64(ik)? != keys.identity.identity_key().serialize().to_vec() {
                return Err(CoreError::internal("server identity key does not match the account key"));
            }
        }
        let account = AccountInfo {
            account_id: resp.account_id,
            number: resp.number,
            device_id: resp.device_id,
            server_url: url.to_string(),
        };
        self.remember_device_name(device_name);
        self.finish_login(account.clone(), resp.token, &secret, registration_id, kdf, &wrapped, true).await?;
        self.lock_after_login(password).await;
        Ok(account)
    }

    pub async fn recover(
        self: &Rc<Self>,
        server_url: &str,
        number: &str,
        mnemonic: &[String],
        new_password: &str,
        device_name: &str,
    ) -> Result<AccountInfo> {
        self.ensure_logged_out()?;
        validate_password(new_password)?;
        let url = normalize_server_url(server_url).ok_or_else(|| CoreError::invalid("invalid server URL"))?;
        let number = normalize_number(number).ok_or_else(|| CoreError::invalid("invalid account number"))?;
        let secret = crypto::parse_mnemonic(mnemonic)?;
        let keys = AccountKeys::derive(&secret)?;
        let api = ApiClient::new(&url, None);
        #[derive(Deserialize)]
        struct Challenge {
            challenge: String,
        }
        let ch: Challenge = api.post("/v1/auth/recover/challenge", &json!({ "number": number })).await?;
        let challenge = unb64(&ch.challenge)?;
        let kdf = KdfParams::new_random();
        let pw = derive_password(new_password.to_string(), kdf.clone()).await?;
        let registration_id = new_registration_id();
        let wrapped = crypto::wrap_account_secret(&pw.wrap_key, &secret)?;
        let req = RecoverReq {
            number,
            challenge: ch.challenge.clone(),
            signature: b64(keys.sign_recovery_challenge(&challenge)),
            kdf: kdf.clone(),
            auth_key: b64(pw.auth_key),
            wrapped_account_key: b64(&wrapped),
            device: device_dto(&self.platform, registration_id),
        };
        let resp: LoginResp = api.post("/v1/auth/recover", &req).await.map_err(|e| {
            if e.code == ErrorCode::Unauthorized || e.code == ErrorCode::Forbidden {
                CoreError::new(ErrorCode::InvalidMnemonic, "the recovery phrase does not match this account")
            } else {
                e
            }
        })?;
        let account = AccountInfo {
            account_id: resp.account_id,
            number: resp.number,
            device_id: resp.device_id,
            server_url: url,
        };
        self.remember_device_name(device_name);
        self.finish_login(account.clone(), resp.token, &secret, registration_id, &kdf, &wrapped, true).await?;
        self.lock_after_login(new_password).await;
        Ok(account)
    }

    pub(super) async fn finish_login(
        self: &Rc<Self>,
        account: AccountInfo,
        token: String,
        secret: &AccountSecret,
        registration_id: u32,
        kdf: &KdfParams,
        wrapped: &[u8],
        restore_history: bool,
    ) -> Result<()> {
        self.set_session(account, token, secret, registration_id)?;
        {
            let db = self.db.borrow();
            db.kv_set_json("kdf", kdf)?;
            // Kept locally only to check the password before showing the recovery phrase.
            db.kv_set("wrapped_account_key", wrapped)?;
            db.kv_set_json("vault_seq", &0i64)?;
            db.kv_set_json("backup_enabled", &true)?;
            db.kv_set_json("restore_pending", &restore_history)?;
        }
        if let Err(e) = self.upload_initial_keys().await {
            // Without keys nobody can message this device; undo the local login.
            let _ = self.session()?.api.delete::<()>(&format!("/v1/devices/{}", self.session()?.account.device_id), None).await;
            let _ = self.wipe_local("logout");
            return Err(e);
        }
        if !restore_history {
            // New account: publish an (empty) profile so contacts can see something.
            let _ = self.publish_profile().await;
        }
        self.clone().start_background();
        Ok(())
    }

    /// Remembers whether the account password is below the current rules (older accounts),
    /// so the app can ask to change it. Only a flag is stored, never the password.
    pub(super) fn note_password_strength(&self, password: &str) {
        let weak = !crate::util::password_problems(password).is_empty();
        let _ = self.db.borrow().kv_set_json("password_weak", &weak);
    }

    /// Locks the local data with the password just used to log in. On failure the app asks
    /// for the password again (AppState.local_protected stays false).
    async fn lock_after_login(&self, password: &str) {
        self.note_password_strength(password);
        if let Err(e) = self.lock_with_password(password).await {
            // The data stays in memory only (nothing unprotected on disk); the app asks for
            // the password again to lock it (AppState.local_protected stays false).
            log::error!("cannot lock the local database: {e}");
        }
    }

    pub async fn logout(&self) -> Result<()> {
        if let Ok(s) = self.session() {
            let _ = s.api.delete::<()>(&format!("/v1/devices/{}", s.account.device_id), None).await;
        }
        self.wipe_local("logout")
    }

    fn stored_kdf(&self) -> Result<KdfParams> {
        self.db.borrow().kv_get_json("kdf")?.ok_or_else(|| CoreError::internal("kdf parameters missing"))
    }

    pub async fn change_password(&self, old: &str, new: &str) -> Result<()> {
        validate_password(new)?;
        let s = self.session()?;
        let old_keys = derive_password(old.to_string(), self.stored_kdf()?).await?;
        let kdf = KdfParams::new_random();
        let new_keys = derive_password(new.to_string(), kdf.clone()).await?;
        let secret = self.account_secret()?;
        let new_wrapped = crypto::wrap_account_secret(&new_keys.wrap_key, &secret)?;
        let r: Result<serde_json::Value> = s
            .api
            .put(
                "/v1/account/password",
                &json!({
                    "auth_key": b64(old_keys.auth_key),
                    "kdf": kdf,
                    "new_auth_key": b64(new_keys.auth_key),
                    "wrapped_account_key": b64(&new_wrapped),
                }),
            )
            .await;
        r.map_err(|e| {
            if e.code == ErrorCode::Unauthorized || e.code == ErrorCode::Forbidden {
                CoreError::new(ErrorCode::InvalidPassword, "current password is wrong")
            } else {
                e
            }
        })?;
        self.db.borrow().kv_set_json("kdf", &kdf)?;
        self.db.borrow().kv_set("wrapped_account_key", &new_wrapped)?;
        self.note_password_strength(new);
        self.rewrap_local_key(new).await
    }

    /// The password was changed on another device, so the local lock still uses the old one.
    fn local_kdf_outdated(&self, dto: &AccountDto) -> bool {
        self.local_protected() && self.password_changed_elsewhere(dto.kdf.as_ref())
    }

    /// Returns the 24-word recovery phrase after checking the password locally.
    pub async fn reveal_mnemonic(&self, password: &str) -> Result<Vec<String>> {
        let wrapped = self
            .db
            .borrow()
            .kv_get("wrapped_account_key")?
            .ok_or_else(|| CoreError::new(ErrorCode::InvalidInput, "log in again on this device to view the recovery phrase"))?;
        let keys = derive_password(password.to_string(), self.stored_kdf()?).await?;
        let secret = crypto::unwrap_account_secret(&keys.wrap_key, &wrapped)?;
        if !crypto::ct_eq(&secret.0, &self.account_secret()?.0) {
            return Err(CoreError::new(ErrorCode::InvalidPassword, "wrong password"));
        }
        Ok(crypto::mnemonic_words(&secret))
    }

    pub async fn delete_account(&self, password: &str) -> Result<()> {
        let s = self.session()?;
        let keys = derive_password(password.to_string(), self.stored_kdf()?).await?;
        s.api
            .delete("/v1/account", Some(&json!({ "auth_key": b64(keys.auth_key) })))
            .await
            .map_err(|e| {
                if e.code == ErrorCode::Unauthorized || e.code == ErrorCode::Forbidden {
                    CoreError::new(ErrorCode::InvalidPassword, "wrong password")
                } else {
                    e
                }
            })?;
        self.wipe_local("account_deleted")
    }

    pub fn account_secret(&self) -> Result<AccountSecret> {
        let v = self.db.borrow().kv_get("account_secret")?.ok_or_else(|| CoreError::internal("no secret"))?;
        Ok(AccountSecret(crypto::key32(&v)?))
    }

    // --- keys -----------------------------------------------------------------

    async fn upload_initial_keys(&self) -> Result<()> {
        let mut req = KeysUploadReq::default();
        req.signed_prekey = Some(self.new_signed_prekey()?);
        req.kyber_last_resort = Some(self.new_kyber_prekey(true)?);
        req.prekeys = self.new_prekeys(PREKEY_BATCH)?;
        req.kyber_prekeys = (0..KYBER_BATCH).map(|_| self.new_kyber_prekey(false)).collect::<Result<_>>()?;
        let _: serde_json::Value = self.session()?.api.put("/v1/keys", &req).await?;
        Ok(())
    }

    fn new_signed_prekey(&self) -> Result<SignedKeyDto> {
        let s = self.session()?;
        let id = self.db.borrow().next_counter("next_signed_prekey_id", rand::rng().random_range(1..1_000_000))? as u32;
        let mut rng = rand::rng();
        let kp = KeyPair::generate(&mut rng);
        let signature = s.keys.identity.private_key().calculate_signature(&kp.public_key.serialize(), &mut rng)?;
        let record = SignedPreKeyRecord::new(
            SignedPreKeyId::from(id),
            Timestamp::from_epoch_millis(now_ms() as u64),
            &kp,
            &signature,
        );
        self.db.borrow().conn.execute(
            "INSERT OR REPLACE INTO signed_prekeys (id, record, created_at) VALUES (?1, ?2, ?3)",
            params![id, record.serialize()?, now_ms()],
        )?;
        self.db.borrow().kv_set_json("current_signed_prekey", &id)?;
        Ok(SignedKeyDto { id, public_key: b64(kp.public_key.serialize()), signature: b64(&signature) })
    }

    fn new_kyber_prekey(&self, last_resort: bool) -> Result<SignedKeyDto> {
        let s = self.session()?;
        let id = self.db.borrow().next_counter("next_kyber_prekey_id", rand::rng().random_range(1..1_000_000))? as u32;
        let record =
            KyberPreKeyRecord::generate(kem::KeyType::Kyber1024, KyberPreKeyId::from(id), s.keys.identity.private_key())?;
        self.db.borrow().conn.execute(
            "INSERT OR REPLACE INTO kyber_prekeys (id, record, last_resort, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![id, record.serialize()?, last_resort as i64, now_ms()],
        )?;
        Ok(SignedKeyDto {
            id,
            public_key: b64(record.public_key()?.serialize()),
            signature: b64(record.signature()?),
        })
    }

    fn new_prekeys(&self, count: u32) -> Result<Vec<PreKeyDto>> {
        let mut out = Vec::with_capacity(count as usize);
        let mut rng = rand::rng();
        for _ in 0..count {
            let id = self.db.borrow().next_counter("next_prekey_id", rand::rng().random_range(1..1_000_000))? as u32;
            let kp = KeyPair::generate(&mut rng);
            let record = PreKeyRecord::new(PreKeyId::from(id), &kp);
            self.db.borrow().conn.execute(
                "INSERT OR REPLACE INTO prekeys (id, record) VALUES (?1, ?2)",
                params![id, record.serialize()?],
            )?;
            out.push(PreKeyDto { id, public_key: b64(kp.public_key.serialize()) });
        }
        Ok(out)
    }

    /// Tops up one-time keys and rotates the signed pre-key when needed.
    pub async fn refresh_keys(&self) -> Result<()> {
        let s = self.session()?;
        let count: KeyCountDto = s.api.get("/v1/keys/count").await?;
        let mut req = KeysUploadReq::default();
        if count.prekeys < PREKEY_LOW {
            req.prekeys = self.new_prekeys(PREKEY_BATCH)?;
        }
        if count.kyber_prekeys < PREKEY_LOW {
            req.kyber_prekeys = (0..KYBER_BATCH).map(|_| self.new_kyber_prekey(false)).collect::<Result<_>>()?;
        }
        let current: Option<u32> = self.db.borrow().kv_get_json("current_signed_prekey")?;
        let created: Option<i64> = match current {
            Some(id) => self
                .db
                .borrow()
                .conn
                .query_row("SELECT created_at FROM signed_prekeys WHERE id = ?1", [id], |r| r.get(0))
                .ok(),
            None => None,
        };
        if created.map(|c| now_ms() - c > SIGNED_PREKEY_ROTATION_MS).unwrap_or(true) {
            req.signed_prekey = Some(self.new_signed_prekey()?);
            req.kyber_last_resort = Some(self.new_kyber_prekey(true)?);
        }
        if req.signed_prekey.is_none() && req.prekeys.is_empty() && req.kyber_prekeys.is_empty() {
            return Ok(());
        }
        let _: serde_json::Value = s.api.put("/v1/keys", &req).await?;
        // Keep old signed/last-resort keys for a month so in-flight messages still decrypt.
        let cutoff = now_ms() - 30 * 24 * 3600 * 1000;
        let db = self.db.borrow();
        let current: Option<u32> = db.kv_get_json("current_signed_prekey")?;
        db.conn.execute(
            "DELETE FROM signed_prekeys WHERE created_at < ?1 AND id != ?2",
            params![cutoff, current.unwrap_or(0)],
        )?;
        db.conn.execute(
            "DELETE FROM kyber_prekeys WHERE last_resort = 1 AND created_at < ?1 AND id != (SELECT MAX(id) FROM kyber_prekeys WHERE last_resort = 1)",
            [cutoff],
        )?;
        Ok(())
    }

    // --- account settings -------------------------------------------------------

    pub fn privacy(&self) -> Result<PrivacySettings> {
        Ok(self.db.borrow().kv_get_json("privacy")?.unwrap_or_default())
    }

    pub async fn account_details(&self) -> Result<AccountDetails> {
        let s = self.session()?;
        let dto: AccountDto = s.api.get("/v1/account").await?;
        let local_password_outdated = self.local_kdf_outdated(&dto);
        let mut privacy = self.privacy()?;
        if privacy.last_seen != dto.privacy.last_seen {
            privacy.last_seen = dto.privacy.last_seen.clone();
            self.db.borrow().kv_set_json("privacy", &privacy)?;
        }
        Ok(AccountDetails {
            account: s.account.clone(),
            username: dto.username,
            phone: dto.phone,
            phone_verified: dto.phone_verified,
            discoverable_by_phone: dto.discoverable_by_phone,
            privacy,
            backup_enabled: self.backup_enabled(),
            totp_enabled: dto.totp_enabled,
            login_approval: dto.login_approval,
            local_password_outdated,
            password_weak: self.db.borrow().kv_get_json("password_weak").ok().flatten().unwrap_or(false),
        })
    }

    /// Starts enabling two-step verification: a new secret to show as a QR code
    /// (`otpauth://` URI) and as text. Enabled only once `totp_enable` confirms a code.
    pub async fn totp_setup(&self) -> Result<serde_json::Value> {
        self.session()?.api.post("/v1/account/totp/setup", &json!({})).await
    }

    pub async fn totp_enable(&self, code: &str) -> Result<()> {
        let _: serde_json::Value = self.session()?.api.post("/v1/account/totp/enable", &json!({ "code": code.trim() })).await?;
        Ok(())
    }

    pub async fn totp_disable(&self, code: &str) -> Result<()> {
        self.session()?.api.delete("/v1/account/totp", Some(&json!({ "code": code.trim() }))).await
    }

    /// When a UI reminder (e.g. "turn on two-step verification") was last shown, in ms. Kept in
    /// the encrypted local database so it survives restarts, unlike the webview's storage.
    pub fn reminder_shown_at(&self, name: &str) -> Result<Option<i64>> {
        self.db.borrow().kv_get_i64(&reminder_key(name)?)
    }

    pub fn mark_reminder_shown(&self, name: &str) -> Result<()> {
        self.db.borrow().kv_set_json(&reminder_key(name)?, &crate::util::now_ms())
    }

    pub fn backup_enabled(&self) -> bool {
        self.db.borrow().kv_get_json("backup_enabled").ok().flatten().unwrap_or(true)
    }

    pub async fn set_privacy(&self, patch: PrivacyPatch) -> Result<PrivacySettings> {
        let s = self.session()?;
        let mut p = self.privacy()?;
        if let Some(ls) = patch.last_seen {
            if ls != "everyone" && ls != "nobody" {
                return Err(CoreError::invalid("last_seen must be everyone or nobody"));
            }
            let _: serde_json::Value = s.api.put("/v1/account/privacy", &json!({ "last_seen": ls })).await?;
            p.last_seen = ls;
        }
        if let Some(v) = patch.read_receipts {
            p.read_receipts = v;
        }
        if let Some(v) = patch.typing_indicators {
            p.typing_indicators = v;
        }
        self.db.borrow().kv_set_json("privacy", &p)?;
        self.vault_put_settings().await;
        Ok(p)
    }

    pub async fn set_backup_enabled(&self, enabled: bool) -> Result<()> {
        self.db.borrow().kv_set_json("backup_enabled", &enabled)?;
        if enabled {
            self.vault_backfill_messages()?;
            self.vault_notify.notify_one();
        }
        Ok(())
    }

    pub async fn request_phone_code(&self, phone: &str) -> Result<()> {
        let phone = crate::util::normalize_phone(phone, None)
            .ok_or_else(|| CoreError::invalid("phone number must be in international format, e.g. +40722123456"))?;
        let _: serde_json::Value = self.session()?.api.post("/v1/account/phone", &json!({ "phone": phone })).await?;
        Ok(())
    }

    pub async fn verify_phone(&self, phone: &str, code: &str) -> Result<()> {
        let phone = crate::util::normalize_phone(phone, None).ok_or_else(|| CoreError::invalid("invalid phone"))?;
        let _: serde_json::Value = self
            .session()?
            .api
            .post("/v1/account/phone/verify", &json!({ "phone": phone, "code": code.trim() }))
            .await?;
        Ok(())
    }

    pub async fn remove_phone(&self) -> Result<()> {
        self.session()?.api.delete::<()>("/v1/account/phone", None).await
    }

    pub async fn set_discoverable(&self, discoverable: bool) -> Result<()> {
        let _: serde_json::Value = self
            .session()?
            .api
            .put("/v1/account/discoverable", &json!({ "discoverable": discoverable }))
            .await?;
        Ok(())
    }

    /// Sets (or removes, with None / "") the public username.
    pub async fn set_username(&self, username: Option<String>) -> Result<Option<String>> {
        let username = username.map(|u| u.trim().trim_start_matches('@').to_string()).filter(|u| !u.is_empty());
        // ASCII only: Unicode lowercasing would fold look-alikes (e.g. the Kelvin sign) onto
        // ASCII letters, so "Kate" written with one would silently become "kate".
        if username.as_deref().is_some_and(|u| !u.is_ascii()) {
            return Err(CoreError::invalid("username must use only the letters a-z, digits, _ and ."));
        }
        let username = username.map(|u| u.to_ascii_lowercase());
        let _: serde_json::Value = self
            .session()?
            .api
            .put("/v1/account/username", &json!({ "username": username }))
            .await
            .map_err(|e| {
                if e.code == ErrorCode::Conflict {
                    CoreError::new(ErrorCode::Conflict, "this username is taken")
                } else {
                    e
                }
            })?;
        Ok(username)
    }

    /// Registers (or clears, with empty strings) this device's push token: provider "fcm"
    /// (messages and calls) or "apns_voip" (iOS PushKit token, incoming calls only; an empty
    /// token removes it). The server stores this account's push tag with it and pushes carry
    /// the tag back, so the app knows which of the accounts on the phone a push is for.
    pub async fn set_push_token(&self, provider: &str, token: &str) -> Result<()> {
        let key = match provider {
            "" | "fcm" => "push_token",
            "apns_voip" => "voip_token",
            _ => return Err(CoreError::invalid("unsupported push provider")),
        };
        let tag = if provider.is_empty() { None } else { Some(self.push_tag()?) };
        let _: serde_json::Value = self
            .session()?
            .api
            .put("/v1/devices/current/push", &json!({ "provider": provider, "token": token, "tag": tag }))
            .await?;
        let db = self.db.borrow();
        db.kv_set_json(key, &token)?;
        if provider.is_empty() {
            db.kv_set_json("voip_token", &"")?;
        }
        Ok(())
    }

    /// This account's push tag on this device: random, created on first use. It only lets the
    /// app match a push to one of its local accounts; it means nothing to anyone else.
    pub fn push_tag(&self) -> Result<String> {
        let db = self.db.borrow();
        if let Some(t) = db.kv_get_json::<String>("push_tag")?.filter(|t| !t.is_empty()) {
            return Ok(t);
        }
        let t = hex::encode(rand::random::<[u8; 16]>());
        db.kv_set_json("push_tag", &t)?;
        Ok(t)
    }

    // --- devices ----------------------------------------------------------------

    pub async fn list_devices(&self) -> Result<Vec<DeviceInfo>> {
        let s = self.session()?;
        let devices = self.fetch_devices().await?;
        Ok(devices
            .into_iter()
            .map(|d| DeviceInfo {
                current: d.device_id == s.account.device_id,
                device_id: d.device_id,
                name: self.device_display_name(&d),
                platform: d.platform,
                created_at: d.created_at,
                last_seen_at: d.last_seen_at,
            })
            .collect())
    }

    pub async fn unlink_device(&self, device_id: u32) -> Result<()> {
        let s = self.session()?;
        if device_id == s.account.device_id {
            return self.logout().await;
        }
        s.api.delete::<()>(&format!("/v1/devices/{device_id}"), None).await?;
        self.db.borrow().conn.execute(
            "DELETE FROM sessions WHERE account_id = ?1 AND device_id = ?2",
            params![s.account.account_id, device_id],
        )?;
        Ok(())
    }

    pub async fn rename_device(&self, name: &str) -> Result<()> {
        if name.trim().is_empty() {
            return Err(CoreError::invalid("name is empty"));
        }
        // Stored encrypted: the server only sees a placeholder.
        self.upload_device_name(name).await
    }
}

fn reminder_key(name: &str) -> Result<String> {
    let valid = !name.is_empty() && name.len() <= 32 && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
    if !valid {
        return Err(CoreError::invalid("invalid reminder name"));
    }
    Ok(format!("reminder_shown:{name}"))
}
