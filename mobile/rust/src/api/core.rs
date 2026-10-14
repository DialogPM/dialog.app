//! Bridge between the Flutter app and the shared Rust core (`chat_core`).
//!
//! The Dart side calls [`invoke`] with the same command names and JSON requests as the
//! desktop app (see `app/src/lib/backend.ts`), and listens to core events through [`events`].
//!
//! Several accounts can live on the phone (local profiles, shared with the desktop app:
//! `chat_core::profiles`). The app starts on a blank profile (the start screen); the profile
//! commands (`profiles_list`, `profile_open`, `profile_close`, ...) go through [`invoke`] like
//! the core commands and swap the active profile's core in place. Only the active profile's
//! core runs (connection, notifications, calls).

use std::sync::{Arc, Mutex, OnceLock};

use chat_core::profiles::Profiles;
use chat_core::EventSink;

use crate::frb_generated::StreamSink;

static PROFILES: OnceLock<Profiles> = OnceLock::new();
static SINK: Mutex<Option<StreamSink<String>>> = Mutex::new(None);

/// Configuration passed by the Flutter app at startup.
pub struct BridgeConfig {
    /// App-private directory: the profiles registry, the profiles' data directories, and the
    /// account of versions before profiles (kept in place as the profile "default").
    pub data_dir: String,
    /// Unused (kept for the generated bindings): every profile's core starts with its own
    /// per-run key, since local data is written to disk only once locked with the password.
    pub db_key_hex: String,
    /// Built-in default server URL.
    pub default_server_url: String,
    /// "android" | "ios" | "desktop"
    pub platform: String,
}

struct FlutterEvents;

impl EventSink for FlutterEvents {
    fn emit(&self, event: &str, payload: serde_json::Value) {
        let msg = serde_json::json!({ "event": event, "payload": payload }).to_string();
        if let Ok(guard) = SINK.lock() {
            if let Some(sink) = guard.as_ref() {
                let _ = sink.add(msg);
            }
        }
    }
}

/// Loads the profiles and starts the blank profile of the start screen (idempotent: later
/// calls are ignored).
pub fn start_core(config: BridgeConfig) -> anyhow::Result<()> {
    if PROFILES.get().is_some() {
        return Ok(());
    }
    let profiles = Profiles::start(config.data_dir.into(), config.default_server_url, config.platform, Arc::new(FlutterEvents))
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    let _ = PROFILES.set(profiles);
    Ok(())
}

/// Stream of core events as JSON strings: `{"event": "<name>", "payload": ...}`.
pub fn events(sink: StreamSink<String>) -> anyhow::Result<()> {
    *SINK.lock().map_err(|_| anyhow::anyhow!("event sink poisoned"))? = Some(sink);
    Ok(())
}

/// Runs a core command on the active profile, or a profile command. `req_json` is the JSON
/// request ("null" when the command has none); returns the JSON result. Errors are strings of
/// the form "<code>: <message>".
pub async fn invoke(cmd: String, req_json: String) -> anyhow::Result<String> {
    let profiles = PROFILES.get().ok_or_else(|| anyhow::anyhow!("internal: core not started"))?;
    let req: serde_json::Value = if req_json.trim().is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_str(&req_json)?
    };
    match profiles.invoke(&cmd, req).await {
        Ok(v) => Ok(v.to_string()),
        Err(e) => Err(anyhow::anyhow!(e)),
    }
}

/// Generates a random 32-byte database key (hex), used on first launch.
#[flutter_rust_bridge::frb(sync)]
pub fn new_db_key() -> String {
    hex::encode(chat_core::crypto::random_bytes::<32>())
}
