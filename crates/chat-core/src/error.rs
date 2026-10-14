use std::fmt;

/// Error type returned by every public core operation.
///
/// It is rendered to the UI as `"<code>: <message>"` (see `app/src/lib/backend.ts`).
#[derive(Debug, Clone)]
pub struct CoreError {
    pub code: ErrorCode,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    Network,
    Unauthorized,
    InvalidPassword,
    InvalidMnemonic,
    NotFound,
    TooManyDevices,
    RegistrationDisabled,
    RateLimited,
    Forbidden,
    InvalidInput,
    DifferentServer,
    Conflict,
    Gone,
    PayloadTooLarge,
    NotLoggedIn,
    /// A device presented an identity key other than the account's pinned one.
    UntrustedIdentity,
    /// Login needs a two-step verification code.
    TotpRequired,
    InvalidTotp,
    /// The group requires members to have a profile name.
    ProfileNameRequired,
    /// A new password does not meet the password rules.
    WeakPassword,
    /// The login waits for approval on one of the account's devices (login_wait_approval).
    ApprovalRequired,
    /// One of the account's devices denied the login.
    LoginDenied,
    /// An admin removed us from the group (or rejected our request): no rejoining by link.
    Banned,
    /// The recipient account exists but has no device right now (logged out everywhere):
    /// messages to it wait in the outbox (engine/send.rs).
    NoDevices,
    /// The recipient account was deleted.
    AccountGone,
    Internal,
}

impl ErrorCode {
    pub fn as_str(&self) -> &'static str {
        match self {
            ErrorCode::Network => "network",
            ErrorCode::Unauthorized => "unauthorized",
            ErrorCode::InvalidPassword => "invalid_password",
            ErrorCode::InvalidMnemonic => "invalid_mnemonic",
            ErrorCode::NotFound => "not_found",
            ErrorCode::TooManyDevices => "too_many_devices",
            ErrorCode::RegistrationDisabled => "registration_disabled",
            ErrorCode::RateLimited => "rate_limited",
            ErrorCode::Forbidden => "forbidden",
            ErrorCode::InvalidInput => "invalid_input",
            ErrorCode::DifferentServer => "different_server",
            ErrorCode::Conflict => "conflict",
            ErrorCode::Gone => "gone",
            ErrorCode::PayloadTooLarge => "payload_too_large",
            ErrorCode::NotLoggedIn => "not_logged_in",
            ErrorCode::UntrustedIdentity => "untrusted_identity",
            ErrorCode::TotpRequired => "totp_required",
            ErrorCode::InvalidTotp => "invalid_totp",
            ErrorCode::ProfileNameRequired => "profile_name_required",
            ErrorCode::WeakPassword => "weak_password",
            ErrorCode::ApprovalRequired => "approval_required",
            ErrorCode::LoginDenied => "login_denied",
            ErrorCode::Banned => "banned",
            ErrorCode::NoDevices => "no_devices",
            ErrorCode::AccountGone => "account_gone",
            ErrorCode::Internal => "internal",
        }
    }
}

impl CoreError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self { code, message: message.into() }
    }
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Internal, message)
    }
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidInput, message)
    }
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::NotFound, message)
    }
    pub fn network(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Network, message)
    }
    pub fn is_network(&self) -> bool {
        self.code == ErrorCode::Network
    }

    /// Prefixes the message with the step that failed, keeping the error code.
    pub fn context(mut self, what: &str) -> Self {
        self.message = format!("{what}: {}", self.message);
        self
    }
}

impl fmt::Display for CoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.message)
    }
}

impl std::error::Error for CoreError {}

pub type Result<T> = std::result::Result<T, CoreError>;

impl From<rusqlite::Error> for CoreError {
    fn from(e: rusqlite::Error) -> Self {
        CoreError::internal(format!("database: {e}"))
    }
}

impl From<serde_json::Error> for CoreError {
    fn from(e: serde_json::Error) -> Self {
        CoreError::internal(format!("json: {e}"))
    }
}

impl From<libsignal_protocol::SignalProtocolError> for CoreError {
    fn from(e: libsignal_protocol::SignalProtocolError) -> Self {
        match e {
            libsignal_protocol::SignalProtocolError::UntrustedIdentity(addr) => CoreError::new(
                ErrorCode::UntrustedIdentity,
                format!("{addr} presented a security key that does not belong to this account"),
            ),
            e => CoreError::internal(format!("signal: {e}")),
        }
    }
}

impl From<libsignal_core::curve::CurveError> for CoreError {
    fn from(e: libsignal_core::curve::CurveError) -> Self {
        CoreError::internal(format!("curve: {e}"))
    }
}

impl From<std::io::Error> for CoreError {
    fn from(e: std::io::Error) -> Self {
        CoreError::internal(format!("io: {e}"))
    }
}

impl From<base64::DecodeError> for CoreError {
    fn from(e: base64::DecodeError) -> Self {
        CoreError::invalid(format!("base64: {e}"))
    }
}

impl From<reqwest::Error> for CoreError {
    fn from(e: reqwest::Error) -> Self {
        CoreError::network(e.to_string())
    }
}

/// Converts any displayable error into an internal error with context.
pub trait Context<T> {
    fn ctx(self, what: &str) -> Result<T>;
}

impl<T, E: fmt::Display> Context<T> for std::result::Result<T, E> {
    fn ctx(self, what: &str) -> Result<T> {
        self.map_err(|e| CoreError::internal(format!("{what}: {e}")))
    }
}
