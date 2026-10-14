//! Call privacy settings (per account, the same on all our devices):
//!
//! * `allow_calls` (default on): when off, one-to-one offers do not ring on any of our devices
//!   and leave no history; the caller's device is told `hangup` with reason `calls_disabled`.
//!   Group calls do not ring either, and this device cannot start, join or place calls.
//! * `relay_only` (default off): every call of ours goes through the TURN relay only
//!   (iceTransportPolicy "relay"), so no peer learns our IP address. The core reports it to the
//!   UIs as `relay` on call events and also drops any non-relay candidate we would send, in
//!   ICE signals and inside the SDP (defense in depth: with the relay policy WebRTC gathers none).
//!
//! Like do not disturb (engine/dnd.rs), the settings reach our other devices through the vault
//! settings record (engine/vault.rs); the newest change wins.

use serde::{Deserialize, Serialize};

use super::Inner;
use crate::error::Result;
use crate::util::now_ms;

const KV_KEY: &str = "call_settings";

/// Stored setting (also the vault copy).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CallSettingsRecord {
    #[serde(default = "default_true")]
    pub allow_calls: bool,
    #[serde(default)]
    pub relay_only: bool,
    /// When it was changed (ms), to keep the newest change across devices; 0 = never set.
    #[serde(default)]
    pub updated_at: i64,
}

fn default_true() -> bool {
    true
}

impl Default for CallSettingsRecord {
    fn default() -> Self {
        CallSettingsRecord { allow_calls: true, relay_only: false, updated_at: 0 }
    }
}

/// Payload of `get_call_settings` / `set_call_settings` and of the `call_settings_changed` event.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct CallSettings {
    pub allow_calls: bool,
    pub relay_only: bool,
}

/// `set_call_settings`: absent fields stay as they are.
#[derive(Debug, Default, Deserialize)]
pub struct CallSettingsPatch {
    pub allow_calls: Option<bool>,
    pub relay_only: Option<bool>,
}

impl From<CallSettingsRecord> for CallSettings {
    fn from(r: CallSettingsRecord) -> Self {
        CallSettings { allow_calls: r.allow_calls, relay_only: r.relay_only }
    }
}

impl Inner {
    pub(super) fn call_settings_record(&self) -> CallSettingsRecord {
        self.db.borrow().kv_get_json(KV_KEY).ok().flatten().unwrap_or_default()
    }

    pub fn get_call_settings(&self) -> CallSettings {
        self.call_settings_record().into()
    }

    /// Calls may ring here and be placed from here.
    pub fn calls_allowed(&self) -> bool {
        self.call_settings_record().allow_calls
    }

    /// Every call of ours goes through the TURN relay only.
    pub fn relay_only(&self) -> bool {
        self.call_settings_record().relay_only
    }

    pub async fn set_call_settings(&self, patch: CallSettingsPatch) -> Result<CallSettings> {
        let cur = self.call_settings_record();
        let r = CallSettingsRecord {
            allow_calls: patch.allow_calls.unwrap_or(cur.allow_calls),
            relay_only: patch.relay_only.unwrap_or(cur.relay_only),
            updated_at: now_ms().max(cur.updated_at + 1),
        };
        self.db.borrow().kv_set_json(KV_KEY, &r)?;
        self.vault_put_settings().await;
        let state = CallSettings::from(r);
        self.emit("call_settings_changed", &state);
        Ok(state)
    }

    /// The settings from another of our devices (vault settings record): newest change wins.
    pub(super) fn apply_synced_call_settings(&self, remote: CallSettingsRecord) -> Result<()> {
        if remote.updated_at <= self.call_settings_record().updated_at {
            return Ok(());
        }
        let before = self.get_call_settings();
        self.db.borrow().kv_set_json(KV_KEY, &remote)?;
        let after = self.get_call_settings();
        if after != before {
            self.emit("call_settings_changed", &after);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{Core, CoreConfig, EventSink};
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct Events(Mutex<Vec<(String, serde_json::Value)>>);
    impl EventSink for Events {
        fn emit(&self, e: &str, v: serde_json::Value) {
            self.0.lock().unwrap().push((e.to_string(), v));
        }
    }

    #[test]
    fn defaults_and_old_records() {
        let d = CallSettingsRecord::default();
        assert!(d.allow_calls && !d.relay_only && d.updated_at == 0);
        // Missing fields (a record from a future or partial writer) keep the safe defaults.
        let r: CallSettingsRecord = serde_json::from_str(r#"{"updated_at":5}"#).unwrap();
        assert_eq!(r, CallSettingsRecord { allow_calls: true, relay_only: false, updated_at: 5 });
    }

    #[tokio::test]
    async fn set_and_sync() {
        let dir = tempfile::tempdir().unwrap();
        let events = Arc::new(Events::default());
        let core = Core::start(CoreConfig {
            data_dir: dir.path().to_path_buf(),
            db_key: [6u8; 32],
            default_server_url: String::new(),
            platform: "test".into(),
            events: events.clone(),
        })
        .unwrap();
        core.run(|i| async move {
            assert_eq!(i.get_call_settings(), CallSettings { allow_calls: true, relay_only: false });
            assert!(i.calls_allowed() && !i.relay_only());

            let st = i.set_call_settings(CallSettingsPatch { allow_calls: Some(false), relay_only: None }).await?;
            assert_eq!(st, CallSettings { allow_calls: false, relay_only: false });
            let st = i.set_call_settings(CallSettingsPatch { allow_calls: None, relay_only: Some(true) }).await?;
            assert_eq!(st, CallSettings { allow_calls: false, relay_only: true }, "a patch keeps the other field");
            assert!(!i.calls_allowed() && i.relay_only());

            // An older change from another device loses; a newer one wins.
            let local = i.call_settings_record();
            i.apply_synced_call_settings(CallSettingsRecord { allow_calls: true, relay_only: false, updated_at: local.updated_at - 1 })?;
            assert!(!i.calls_allowed());
            i.apply_synced_call_settings(CallSettingsRecord { allow_calls: true, relay_only: false, updated_at: local.updated_at + 1 })?;
            assert!(i.calls_allowed() && !i.relay_only());
            // The same state again changes nothing visible.
            i.apply_synced_call_settings(CallSettingsRecord { allow_calls: true, relay_only: false, updated_at: local.updated_at + 2 })?;
            i.set_call_settings(CallSettingsPatch::default()).await?;
            assert!(i.call_settings_record().updated_at > local.updated_at + 2, "a local change is always the newest");
            Ok(())
        })
        .await
        .unwrap();
        let changes = events.0.lock().unwrap().iter().filter(|(e, _)| e == "call_settings_changed").count();
        assert_eq!(changes, 4, "every set, and the synced change that changed something");
    }
}
