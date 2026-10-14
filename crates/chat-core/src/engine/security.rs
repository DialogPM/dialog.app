//! Account security: alerts about newly linked devices, device names encrypted for the
//! account's own devices, approval of new password logins, spam reports and the
//! registration proof-of-work.

use std::rc::Rc;
use std::time::Duration;

use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use zeroize::Zeroizing;

use super::Inner;
use crate::api::{ApiClient, ApiErrorOrNet, DeviceDto, DeviceInfoDto, LoginResp};
use crate::crypto::{self, b64, unb64, KdfParams};
use crate::error::{CoreError, ErrorCode, Result};
use crate::model::{AccountInfo, Contact};

/// What the server stores as the name of a device whose real name is encrypted.
pub(super) const DEVICE_NAME_PLACEHOLDER: &str = "Dialog";
/// (device id, created_at) of the account's devices at the last check.
const KNOWN_DEVICES_KEY: &str = "known_devices";
const DEVICE_ALERTS_KEY: &str = "device_alerts";
/// This device's real name, until it is stored encrypted on the server.
const DEVICE_NAME_KEY: &str = "device_name";
const MAX_DEVICE_ALERTS: usize = 20;
/// A server asking for more work than this is refused (it would freeze the app).
const MAX_POW_BITS: u32 = 28;

/// A device that joined the account after this one (shown until dismissed).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NewDeviceAlert {
    pub device_id: u32,
    pub name: String,
    pub platform: String,
    pub created_at: i64,
}

/// A password login waiting for approval by one of our devices.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginRequestInfo {
    pub id: String,
    pub name: String,
    pub platform: String,
    pub created_at: i64,
    pub expires_at: i64,
}

/// A password login of this device waiting for approval (see `login_wait_approval`).
pub struct PendingLogin {
    pub url: String,
    pub request_id: String,
    pub secret: String,
    pub wrap_key: Zeroizing<[u8; 32]>,
    pub kdf: KdfParams,
    pub registration_id: u32,
    pub password: Zeroizing<String>,
    pub device_name: String,
}

/// Trims a device name the way it is stored (at most 64 characters, "Dialog" when empty).
pub(super) fn clean_device_name(name: &str) -> String {
    let name: String = name.trim().chars().take(64).collect();
    if name.is_empty() {
        DEVICE_NAME_PLACEHOLDER.into()
    } else {
        name
    }
}

/// The device as announced to the server: the real name is uploaded encrypted afterwards.
pub(super) fn placeholder_device(platform: &str, registration_id: u32) -> DeviceDto {
    DeviceDto { name: DEVICE_NAME_PLACEHOLDER.into(), platform: platform.to_string(), registration_id }
}

impl Inner {
    // --- device names -------------------------------------------------------------------

    /// Remembers this device's name; it is uploaded encrypted at the next device check.
    pub(super) fn remember_device_name(&self, name: &str) {
        let _ = self.db.borrow().kv_set_json(DEVICE_NAME_KEY, &clean_device_name(name));
    }

    /// The readable name of a device: decrypted when encrypted, else the stored plaintext
    /// (devices that have not uploaded an encrypted name yet).
    pub(super) fn device_display_name(&self, d: &DeviceInfoDto) -> String {
        let decrypted = d.encrypted_name.as_deref().and_then(|e| {
            let s = self.session().ok()?;
            crypto::decrypt_device_name(&s.keys.device_name_key, &unb64(e).ok()?).ok()
        });
        decrypted.unwrap_or_else(|| d.name.clone())
    }

    /// Stores this device's name on the server, encrypted for the account's devices.
    pub(super) async fn upload_device_name(&self, name: &str) -> Result<()> {
        let s = self.session()?;
        let name = clean_device_name(name);
        let enc = crypto::encrypt_device_name(&s.keys.device_name_key, &name)?;
        let _: Value = s
            .api
            .put("/v1/devices/current", &json!({ "name": DEVICE_NAME_PLACEHOLDER, "encrypted_name": b64(&enc) }))
            .await?;
        self.db.borrow().kv_delete(DEVICE_NAME_KEY)?;
        Ok(())
    }

    pub(super) async fn fetch_devices(&self) -> Result<Vec<DeviceInfoDto>> {
        #[derive(Deserialize)]
        struct Resp {
            devices: Vec<DeviceInfoDto>,
        }
        let r: Resp = self.session()?.api.get("/v1/devices").await?;
        Ok(r.devices)
    }

    // --- new-device alerts --------------------------------------------------------------

    /// Compares the account's devices with the last check and records an alert for each
    /// device added since. The first check after this device joined only records the list
    /// (no alert for devices that existed before, nor for this device itself). Also uploads
    /// this device's encrypted name when the server does not have it yet.
    pub async fn check_devices(self: &Rc<Self>) -> Result<()> {
        let devices = self.fetch_devices().await?;
        let my_id = self.session()?.account.device_id;
        if let Some(me) = devices.iter().find(|d| d.device_id == my_id) {
            if me.encrypted_name.is_none() {
                let pending: Option<String> = self.db.borrow().kv_get_json(DEVICE_NAME_KEY)?;
                let name = pending.unwrap_or_else(|| me.name.clone());
                if let Err(e) = self.upload_device_name(&name).await {
                    log::warn!("upload encrypted device name: {e}");
                }
            }
        }
        let current: Vec<(u32, i64)> = devices.iter().map(|d| (d.device_id, d.created_at)).collect();
        let known: Option<Vec<(u32, i64)>> = self.db.borrow().kv_get_json(KNOWN_DEVICES_KEY)?;
        if let Some(known) = known {
            let mut alerts = self.device_alerts()?;
            let before = alerts.clone();
            for d in &devices {
                let key = (d.device_id, d.created_at);
                if d.device_id == my_id || known.contains(&key) {
                    continue;
                }
                if !alerts.iter().any(|a| (a.device_id, a.created_at) == key) {
                    alerts.push(NewDeviceAlert {
                        device_id: d.device_id,
                        name: self.device_display_name(d),
                        platform: d.platform.clone(),
                        created_at: d.created_at,
                    });
                }
            }
            // A new device uploads its encrypted name right after joining: refresh names.
            for a in alerts.iter_mut() {
                if let Some(d) = devices.iter().find(|d| (d.device_id, d.created_at) == (a.device_id, a.created_at)) {
                    a.name = self.device_display_name(d);
                }
            }
            if alerts.len() > MAX_DEVICE_ALERTS {
                alerts.drain(..alerts.len() - MAX_DEVICE_ALERTS);
            }
            if alerts != before {
                self.db.borrow().kv_set_json(DEVICE_ALERTS_KEY, &alerts)?;
                self.emit("device_alerts", &alerts);
            }
        }
        self.db.borrow().kv_set_json(KNOWN_DEVICES_KEY, &current)?;
        Ok(())
    }

    /// Devices added to the account since this device joined, not yet dismissed.
    pub fn device_alerts(&self) -> Result<Vec<NewDeviceAlert>> {
        Ok(self.db.borrow().kv_get_json(DEVICE_ALERTS_KEY)?.unwrap_or_default())
    }

    pub fn dismiss_device_alerts(&self) -> Result<()> {
        self.db.borrow().kv_delete(DEVICE_ALERTS_KEY)?;
        self.emit("device_alerts", &Vec::<NewDeviceAlert>::new());
        Ok(())
    }

    /// Runs the device check in the background (after devices_changed or a reconnect).
    pub(super) fn spawn_device_check(self: &Rc<Self>) {
        let me = self.clone();
        tokio::task::spawn_local(async move {
            if let Err(e) = me.check_devices().await {
                log::warn!("device check: {e}");
            }
        });
    }

    // --- approval of new logins -----------------------------------------------------------

    /// Remembers a login the server put on hold until one of our devices approves it.
    pub(super) fn hold_pending_login(&self, pending: PendingLogin) -> CoreError {
        *self.pending_login.borrow_mut() = Some(pending);
        CoreError::new(ErrorCode::ApprovalRequired, "approve this login on one of your other devices")
    }

    /// Waits until the held login is approved (then finishes it), denied or expired.
    pub async fn login_wait_approval(self: &Rc<Self>) -> Result<AccountInfo> {
        loop {
            let (url, request_id, secret) = match self.pending_login.borrow().as_ref() {
                Some(p) => (p.url.clone(), p.request_id.clone(), p.secret.clone()),
                None => return Err(CoreError::new(ErrorCode::Gone, "no login is waiting for approval")),
            };
            let api = ApiClient::new(&url, None);
            let r: std::result::Result<Value, ApiErrorOrNet> = api
                .send_json(
                    Method::POST,
                    "/v1/auth/login/poll",
                    Some(&json!({ "request_id": request_id, "secret": secret })),
                )
                .await;
            // Cancelled (or replaced) while we waited.
            if self.pending_login.borrow().as_ref().map(|p| p.request_id != request_id).unwrap_or(true) {
                return Err(CoreError::new(ErrorCode::Gone, "the login was cancelled"));
            }
            match r {
                Ok(v) if v.get("status").and_then(Value::as_str) == Some("approved") => {
                    let resp: LoginResp =
                        serde_json::from_value(v).map_err(|e| CoreError::internal(format!("decode login: {e}")))?;
                    let p = self.pending_login.borrow_mut().take().expect("checked above");
                    return self
                        .complete_password_login(&p.url, resp, &p.wrap_key, &p.kdf, p.registration_id, &p.password, &p.device_name)
                        .await;
                }
                Ok(_) => {} // still pending
                Err(ApiErrorOrNet::Api(e)) => {
                    *self.pending_login.borrow_mut() = None;
                    let err: CoreError = e.into();
                    return Err(match err.code {
                        ErrorCode::LoginDenied => err,
                        ErrorCode::Gone | ErrorCode::NotFound => {
                            CoreError::new(ErrorCode::Gone, "the login was not approved in time")
                        }
                        _ => err,
                    });
                }
                Err(ApiErrorOrNet::Net(e)) => {
                    log::info!("login approval poll: {e}");
                    tokio::time::sleep(Duration::from_secs(3)).await;
                }
            }
        }
    }

    pub fn login_cancel_approval(&self) {
        *self.pending_login.borrow_mut() = None;
    }

    /// Logins of this account waiting for approval by one of its devices.
    pub async fn list_login_requests(&self) -> Result<Vec<LoginRequestInfo>> {
        #[derive(Deserialize)]
        struct Resp {
            requests: Vec<LoginRequestInfo>,
        }
        let r: Resp = self.session()?.api.get("/v1/account/login-requests").await?;
        Ok(r.requests)
    }

    pub async fn answer_login_request(&self, id: &str, approve: bool) -> Result<()> {
        if id.len() != 32 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(CoreError::invalid("invalid login request id"));
        }
        let _: Value = self
            .session()?
            .api
            .post(&format!("/v1/account/login-requests/{id}"), &json!({ "approve": approve }))
            .await?;
        self.refresh_login_requests().await;
        Ok(())
    }

    /// Emits `login_requests` with the pending logins (empty list when none or unsupported).
    pub(super) async fn refresh_login_requests(&self) {
        match self.list_login_requests().await {
            Ok(list) => self.emit("login_requests", &list),
            Err(e) if e.code == ErrorCode::NotFound => {}
            Err(e) => log::info!("login requests: {e}"),
        }
    }

    pub(super) fn spawn_login_requests_refresh(self: &Rc<Self>) {
        let me = self.clone();
        tokio::task::spawn_local(async move { me.refresh_login_requests().await });
    }

    /// Turns "require approval for new logins" on or off.
    pub async fn set_login_approval(&self, enabled: bool) -> Result<()> {
        let _: Value = self.session()?.api.put("/v1/account/login-approval", &json!({ "enabled": enabled })).await?;
        Ok(())
    }

    // --- abuse ------------------------------------------------------------------------------

    /// Reports an account for spam (stored on the server for review) and, with `block`,
    /// blocks it.
    pub async fn report_spam(&self, account_id: &str, block: bool) -> Result<Contact> {
        let s = self.session()?;
        if account_id == s.account.account_id {
            return Err(CoreError::invalid("cannot report your own account"));
        }
        let _: Value = s.api.post("/v1/report", &json!({ "account_id": account_id })).await?;
        if block {
            // Blocking needs the contact row (it may not exist for a stranger's request).
            self.ensure_contact(account_id).await?;
            self.set_blocked(account_id, true)
        } else {
            self.contact_model(account_id)
        }
    }

    /// Solves the server's registration proof-of-work, if it asks for one. Older servers
    /// without the endpoint need none.
    pub(super) async fn registration_pow(&self, api: &ApiClient) -> Result<Option<Value>> {
        #[derive(Deserialize)]
        struct Challenge {
            #[serde(default)]
            challenge: Option<String>,
            #[serde(default)]
            difficulty: u32,
        }
        let ch: Challenge = match api.post("/v1/accounts/challenge", &json!({})).await {
            Ok(c) => c,
            Err(e) if e.code == ErrorCode::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        let Some(challenge_b64) = ch.challenge.filter(|_| ch.difficulty > 0) else {
            return Ok(None);
        };
        if ch.difficulty > MAX_POW_BITS {
            return Err(CoreError::internal("the server asks for too much registration work"));
        }
        let challenge = unb64(&challenge_b64)?;
        let bits = ch.difficulty;
        let nonce = tokio::task::spawn_blocking(move || crypto::solve_pow(&challenge, bits))
            .await
            .map_err(|e| CoreError::internal(format!("proof-of-work task: {e}")))?;
        Ok(Some(json!({ "challenge": challenge_b64, "nonce": nonce })))
    }
}
