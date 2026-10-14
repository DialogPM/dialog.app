//! HTTP client for the server REST API (docs/API.md).

use std::time::Duration;

use reqwest::{Method, StatusCode};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::crypto::KdfParams;
use crate::error::{CoreError, ErrorCode, Result};

#[derive(Clone)]
pub struct ApiClient {
    pub base: String,
    http: reqwest::Client,
    /// Separate connection pool for anonymous (sealed sender) requests, so they never share a
    /// connection with requests that carry our token.
    sealed_http: reqwest::Client,
    token: Option<String>,
}

/// Error returned by the server, with the raw body kept for 409/410 handling.
#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: String,
    pub message: String,
    pub body: Value,
}

impl From<ApiError> for CoreError {
    fn from(e: ApiError) -> Self {
        let code = match e.status {
            StatusCode::UNAUTHORIZED if e.code == "totp_required" => ErrorCode::TotpRequired,
            StatusCode::UNAUTHORIZED if e.code == "invalid_totp" => ErrorCode::InvalidTotp,
            StatusCode::UNAUTHORIZED => ErrorCode::Unauthorized,
            StatusCode::NOT_FOUND => ErrorCode::NotFound,
            StatusCode::TOO_MANY_REQUESTS => ErrorCode::RateLimited,
            StatusCode::PAYLOAD_TOO_LARGE => ErrorCode::PayloadTooLarge,
            StatusCode::GONE => ErrorCode::Gone,
            StatusCode::FORBIDDEN if e.code == "registration_disabled" => ErrorCode::RegistrationDisabled,
            StatusCode::FORBIDDEN if e.code == "login_denied" => ErrorCode::LoginDenied,
            StatusCode::FORBIDDEN if e.code == "banned" => ErrorCode::Banned,
            StatusCode::FORBIDDEN => ErrorCode::Forbidden,
            StatusCode::CONFLICT if e.code == "too_many_devices" => ErrorCode::TooManyDevices,
            StatusCode::CONFLICT => ErrorCode::Conflict,
            StatusCode::BAD_REQUEST => ErrorCode::InvalidInput,
            s if s.is_server_error() => ErrorCode::Network,
            _ => ErrorCode::Internal,
        };
        let msg = if e.message.is_empty() { format!("server returned {}", e.status) } else { e.message };
        CoreError::new(code, msg)
    }
}

pub type ApiResult<T> = std::result::Result<T, ApiErrorOrNet>;

/// Either a structured server error or a transport failure.
#[derive(Debug)]
pub enum ApiErrorOrNet {
    Api(ApiError),
    Net(CoreError),
}

impl From<ApiErrorOrNet> for CoreError {
    fn from(e: ApiErrorOrNet) -> Self {
        match e {
            ApiErrorOrNet::Api(a) => a.into(),
            ApiErrorOrNet::Net(n) => n,
        }
    }
}

impl From<reqwest::Error> for ApiErrorOrNet {
    fn from(e: reqwest::Error) -> Self {
        ApiErrorOrNet::Net(CoreError::network(e.to_string()))
    }
}

// ---------------------------------------------------------------------------
// Wire types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerInfoDto {
    pub name: String,
    pub version: String,
    pub registration_enabled: bool,
    #[serde(default)]
    pub phone_verification_enabled: bool,
    pub max_attachment_bytes: i64,
    pub max_devices: i64,
    /// Public key that signs the sender certificates of sealed messages.
    #[serde(default)]
    pub sealed_sender_trust_root: Option<String>,
    /// zkgroup server public parameters (private groups); absent on servers without them.
    #[serde(default)]
    pub zkgroup_public_params: Option<String>,
    /// The server has the private groups v2 API (docs/ZKGROUP-DESIGN.md).
    #[serde(default)]
    pub groups_v2: bool,
    /// The server wants new groups created as v2.
    #[serde(default)]
    pub groups_v2_default: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeviceDto {
    pub name: String,
    pub platform: String,
    pub registration_id: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct RegisterReq {
    pub kdf: KdfParams,
    pub auth_key: String,
    pub wrapped_account_key: String,
    pub identity_key: String,
    pub recovery_public_key: String,
    pub device: DeviceDto,
    /// Registration proof-of-work ({challenge, nonce}) when the server asks for one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pow: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RegisterResp {
    pub account_id: String,
    pub number: String,
    pub device_id: u32,
    pub token: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LoginResp {
    pub account_id: String,
    pub number: String,
    pub device_id: u32,
    pub token: String,
    pub wrapped_account_key: String,
    #[serde(default)]
    pub identity_key: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RecoverReq {
    pub number: String,
    pub challenge: String,
    pub signature: String,
    pub kdf: KdfParams,
    pub auth_key: String,
    pub wrapped_account_key: String,
    pub device: DeviceDto,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PrivacyDto {
    pub last_seen: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AccountDto {
    pub account_id: String,
    pub number: String,
    #[serde(default)]
    pub phone: Option<String>,
    #[serde(default)]
    pub phone_verified: bool,
    #[serde(default)]
    pub discoverable_by_phone: bool,
    pub privacy: PrivacyDto,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub totp_enabled: bool,
    /// Current password parameters (changed when the password changes on any device).
    #[serde(default)]
    pub kdf: Option<crate::crypto::KdfParams>,
    #[serde(default)]
    pub wrapped_account_key: Option<String>,
    /// New password logins need approval from one of the account's devices.
    #[serde(default)]
    pub login_approval: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DeviceInfoDto {
    pub device_id: u32,
    pub name: String,
    pub platform: String,
    pub created_at: i64,
    pub last_seen_at: i64,
    #[serde(default)]
    pub current: bool,
    /// Real name encrypted with the account's device-name key (`name` is then a placeholder).
    #[serde(default)]
    pub encrypted_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedKeyDto {
    pub id: u32,
    pub public_key: String,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreKeyDto {
    pub id: u32,
    pub public_key: String,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct KeysUploadReq {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signed_prekey: Option<SignedKeyDto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kyber_last_resort: Option<SignedKeyDto>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub prekeys: Vec<PreKeyDto>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub kyber_prekeys: Vec<SignedKeyDto>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct KeyCountDto {
    pub prekeys: i64,
    pub kyber_prekeys: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DeviceBundleDto {
    pub device_id: u32,
    pub registration_id: u32,
    pub signed_prekey: SignedKeyDto,
    pub kyber_prekey: SignedKeyDto,
    #[serde(default)]
    pub prekey: Option<PreKeyDto>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BundlesDto {
    pub identity_key: String,
    pub devices: Vec<DeviceBundleDto>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DirectoryEntryDto {
    pub account_id: String,
    pub number: String,
    pub identity_key: String,
    #[serde(default)]
    pub username: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PhoneMatchDto {
    pub hash: String,
    pub account_id: String,
    pub number: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProfileDto {
    pub data: String,
    #[serde(default)]
    pub avatar_id: Option<String>,
    #[serde(default)]
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct OutgoingMessageDto {
    pub device_id: u32,
    pub registration_id: u32,
    #[serde(rename = "type")]
    pub kind: u8,
    pub content: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SendReq {
    pub timestamp: i64,
    pub online: bool,
    pub messages: Vec<OutgoingMessageDto>,
    /// Set on an incoming-call offer: offline devices of the callee get a call push carrying
    /// only this id (server API section 14). The content itself stays encrypted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub call_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct RecipientDto {
    pub account_id: String,
    pub device_id: u32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub registration_id: u32,
}

fn is_zero(v: &u32) -> bool {
    *v == 0
}

#[derive(Debug, Clone, Serialize)]
pub struct GroupSendReq {
    pub timestamp: i64,
    pub online: bool,
    pub content: String,
    pub recipients: Vec<RecipientDto>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Envelope {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: u8,
    pub source_account: String,
    pub source_device: u32,
    #[serde(default)]
    pub destination_device: u32,
    #[serde(default)]
    pub group_id: Option<String>,
    pub timestamp: i64,
    #[serde(default)]
    pub server_timestamp: i64,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupSettingsDto {
    pub only_admins_send: bool,
    pub only_admins_edit: bool,
    #[serde(default)]
    pub disappearing_seconds: i64,
    #[serde(default)]
    pub hide_members: bool,
    #[serde(default)]
    pub member_names: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GroupMemberDto {
    pub account_id: String,
    pub role: String,
    #[serde(default)]
    pub joined_at: i64,
    #[serde(default)]
    pub added_by: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GroupDto {
    pub group_id: String,
    pub revision: i64,
    pub metadata: String,
    #[serde(default)]
    pub avatar_id: Option<String>,
    #[serde(default)]
    pub created_by: Option<String>,
    #[serde(default)]
    pub created_at: i64,
    pub settings: GroupSettingsDto,
    pub members: Vec<GroupMemberDto>,
    /// The group's invite link, when it has one.
    #[serde(default)]
    pub invite: Option<GroupInviteDto>,
    /// Pending join requests (admins only, 0 for members).
    #[serde(default)]
    pub join_requests: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GroupInviteDto {
    pub approval: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GroupRefDto {
    pub group_id: String,
    pub revision: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultRecordDto {
    pub id: String,
    pub data: Option<String>,
    #[serde(default, skip_serializing)]
    pub seq: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct VaultPageDto {
    pub records: Vec<VaultRecordDto>,
    pub more: bool,
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

impl ApiClient {
    pub fn new(base: &str, token: Option<String>) -> Self {
        let build = || {
            reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(60))
                .user_agent(concat!("Dialog/", env!("CARGO_PKG_VERSION")))
                .build()
                .expect("http client")
        };
        Self { base: base.trim_end_matches('/').to_string(), http: build(), sealed_http: build(), token }
    }

    pub fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }

    pub fn with_token(&self, token: String) -> Self {
        Self { base: self.base.clone(), http: self.http.clone(), sealed_http: self.sealed_http.clone(), token: Some(token) }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    fn request(&self, method: Method, path: &str) -> reqwest::RequestBuilder {
        let mut rb = self.http.request(method, self.url(path));
        if let Some(t) = &self.token {
            rb = rb.bearer_auth(t);
        }
        rb
    }

    async fn check(resp: reqwest::Response) -> ApiResult<reqwest::Response> {
        let status = resp.status();
        if status.is_success() {
            return Ok(resp);
        }
        let body: Value = resp.json().await.unwrap_or(Value::Null);
        Err(ApiErrorOrNet::Api(ApiError {
            status,
            code: body.get("error").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
            message: body.get("message").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
            body,
        }))
    }

    pub async fn send_json<B: Serialize + ?Sized, T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<&B>,
    ) -> ApiResult<T> {
        let mut rb = self.request(method, path);
        if let Some(b) = body {
            rb = rb.json(b);
        }
        let resp = Self::check(rb.send().await?).await?;
        if resp.status() == StatusCode::NO_CONTENT {
            return serde_json::from_value(Value::Null)
                .map_err(|e| ApiErrorOrNet::Net(CoreError::internal(format!("decode: {e}"))));
        }
        let bytes = resp.bytes().await?;
        let v: Value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes)
                .map_err(|e| ApiErrorOrNet::Net(CoreError::internal(format!("decode: {e}"))))?
        };
        serde_json::from_value(v).map_err(|e| ApiErrorOrNet::Net(CoreError::internal(format!("decode {path}: {e}"))))
    }

    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        Ok(self.send_json::<(), T>(Method::GET, path, None).await?)
    }

    pub async fn post<B: Serialize + ?Sized, T: DeserializeOwned>(&self, path: &str, body: &B) -> Result<T> {
        Ok(self.send_json(Method::POST, path, Some(body)).await?)
    }

    pub async fn put<B: Serialize + ?Sized, T: DeserializeOwned>(&self, path: &str, body: &B) -> Result<T> {
        Ok(self.send_json(Method::PUT, path, Some(body)).await?)
    }

    pub async fn delete<B: Serialize + ?Sized>(&self, path: &str, body: Option<&B>) -> Result<()> {
        let _: Value = self.send_json(Method::DELETE, path, body).await?;
        Ok(())
    }

    // --- specific endpoints -------------------------------------------------

    pub async fn info(&self) -> Result<ServerInfoDto> {
        self.get("/v1/info").await
    }

    /// Anonymous request authorized by the recipient's unidentified access key.
    async fn sealed_json<B: Serialize + ?Sized, T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        access_key: &[u8; 16],
        body: Option<&B>,
    ) -> ApiResult<T> {
        let mut rb = self.sealed_http.request(method, self.url(path)).header("Unidentified-Access-Key", crate::crypto::b64(access_key));
        if let Some(b) = body {
            rb = rb.json(b);
        }
        let resp = Self::check(rb.send().await?).await?;
        let bytes = resp.bytes().await?;
        let v: Value = if bytes.is_empty() { Value::Null } else { serde_json::from_slice(&bytes).unwrap_or(Value::Null) };
        serde_json::from_value(v).map_err(|e| ApiErrorOrNet::Net(CoreError::internal(format!("decode {path}: {e}"))))
    }

    /// Anonymous private-group (v2) request, authorized by a zkgroup presentation in the
    /// Group-Auth header (never the account token: the server must not link us to the group).
    pub async fn group_v2_json<T: DeserializeOwned>(&self, method: Method, path: &str, auth: &str, body: Option<&Value>) -> ApiResult<T> {
        let mut rb = self.sealed_http.request(method, self.url(path)).header("Group-Auth", auth);
        if let Some(b) = body {
            rb = rb.json(b);
        }
        let resp = Self::check(rb.send().await?).await?;
        let bytes = resp.bytes().await?;
        let v: Value = if bytes.is_empty() { Value::Null } else { serde_json::from_slice(&bytes).unwrap_or(Value::Null) };
        serde_json::from_value(v).map_err(|e| ApiErrorOrNet::Net(CoreError::internal(format!("decode {path}: {e}"))))
    }

    pub async fn put_sealed_messages(&self, account_id: &str, access_key: &[u8; 16], req: &SendReq) -> ApiResult<Value> {
        self.sealed_json(Method::PUT, &format!("/v1/sealed/messages/{account_id}"), access_key, Some(req)).await
    }

    pub async fn get_sealed_keys(&self, account_id: &str, device: &str, access_key: &[u8; 16]) -> ApiResult<BundlesDto> {
        self.sealed_json::<(), _>(Method::GET, &format!("/v1/sealed/keys/{account_id}/{device}"), access_key, None).await
    }

    /// Unauthenticated GET on the anonymous connection pool (key transparency lookups must not
    /// tell the server who is asking).
    pub async fn get_anonymous<T: DeserializeOwned>(&self, path: &str) -> ApiResult<T> {
        let resp = Self::check(self.sealed_http.get(self.url(path)).send().await?).await?;
        let bytes = resp.bytes().await?;
        serde_json::from_slice(&bytes).map_err(|e| ApiErrorOrNet::Net(CoreError::internal(format!("decode {path}: {e}"))))
    }

    pub async fn put_messages(&self, account_id: &str, req: &SendReq) -> ApiResult<Value> {
        self.send_json(Method::PUT, &format!("/v1/messages/{account_id}"), Some(req)).await
    }

    pub async fn put_group_messages(&self, group_id: &str, req: &GroupSendReq) -> ApiResult<Value> {
        self.send_json(Method::PUT, &format!("/v1/groups/{group_id}/messages"), Some(req)).await
    }

    pub async fn upload_attachment(&self, data: Vec<u8>, kind: &str) -> Result<String> {
        #[derive(Deserialize)]
        struct Created {
            id: String,
        }
        let created: Created = self
            .post("/v1/attachments", &serde_json::json!({ "size": data.len(), "kind": kind }))
            .await
            .map_err(|e| CoreError::from(e).context("creating the attachment (POST /v1/attachments)"))?;
        let upload = async {
            let resp = self
                .request(Method::PUT, &format!("/v1/attachments/{}", created.id))
                .timeout(Duration::from_secs(600))
                .header("Content-Type", "application/octet-stream")
                .body(data)
                .send()
                .await?;
            Self::check(resp).await
        };
        upload.await.map_err(|e| CoreError::from(e).context("uploading the attachment (PUT /v1/attachments)"))?;
        Ok(created.id)
    }

    pub async fn download_attachment(&self, id: &str, max: usize) -> Result<Vec<u8>> {
        if !id.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err(CoreError::invalid("bad attachment id"));
        }
        let resp = self
            .request(Method::GET, &format!("/v1/attachments/{id}"))
            .timeout(Duration::from_secs(600))
            .send()
            .await?;
        let mut resp = Self::check(resp).await?;
        let mut out = Vec::new();
        while let Some(chunk) = resp.chunk().await? {
            out.extend_from_slice(&chunk);
            if out.len() > max {
                return Err(CoreError::new(ErrorCode::PayloadTooLarge, "attachment too large"));
            }
        }
        Ok(out)
    }

    /// WebSocket URL for this server (http→ws, https→wss).
    pub fn ws_url(&self) -> String {
        let base = if let Some(rest) = self.base.strip_prefix("https://") {
            format!("wss://{rest}")
        } else if let Some(rest) = self.base.strip_prefix("http://") {
            format!("ws://{rest}")
        } else {
            self.base.clone()
        };
        format!("{base}/v1/ws")
    }
}
