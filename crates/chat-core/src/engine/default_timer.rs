//! Default disappearing-messages timer: every conversation the user starts gets it from the
//! start (like WhatsApp's "Default message timer"):
//!
//! * a direct conversation newly created by us (`open_direct_conversation`: new chat, contact,
//!   QR code, username link...) gets the timer and announces it to the peer exactly like
//!   `set_conversation_settings` (timer update + "disappearing_changed" system message),
//! * a group we create starts with it as its group timer (unless the request chooses one).
//!
//! Conversations started by others, and conversations that already exist, are not affected.
//! The setting is per account (0 = off, the default) and reaches our other devices through the
//! vault settings record (engine/vault.rs); the newest change wins, like engine/dnd.rs.

use serde::{Deserialize, Serialize};

use super::Inner;
use crate::error::{CoreError, Result};
use crate::util::now_ms;

/// The timers the user can choose from (seconds; 0 = off), the same as the apps offer.
pub const DEFAULT_TIMER_OPTIONS: [i64; 9] = [0, 600, 3600, 3 * 3600, 6 * 3600, 12 * 3600, 86400, 7 * 86400, 90 * 86400];
const KV_KEY: &str = "default_disappearing";

/// A valid choice (anything else is refused).
pub fn valid_default_timer(seconds: i64) -> bool {
    DEFAULT_TIMER_OPTIONS.contains(&seconds)
}

/// Stored setting (also the vault copy).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct DefaultTimerRecord {
    #[serde(default)]
    pub seconds: i64,
    /// When it was changed (ms), to keep the newest change across devices; 0 = never set.
    #[serde(default)]
    pub updated_at: i64,
}

/// Payload of `get_default_disappearing` / `set_default_disappearing` and of the
/// `default_disappearing_changed` event.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DefaultDisappearing {
    pub seconds: i64,
    pub options: Vec<i64>,
}

impl Inner {
    pub(super) fn default_timer_record(&self) -> DefaultTimerRecord {
        self.db.borrow().kv_get_json(KV_KEY).ok().flatten().unwrap_or_default()
    }

    /// The timer for conversations we start (0 = off; a damaged value counts as off).
    pub fn default_disappearing_seconds(&self) -> i64 {
        let s = self.default_timer_record().seconds;
        if valid_default_timer(s) {
            s
        } else {
            0
        }
    }

    pub fn get_default_disappearing(&self) -> DefaultDisappearing {
        DefaultDisappearing { seconds: self.default_disappearing_seconds(), options: DEFAULT_TIMER_OPTIONS.to_vec() }
    }

    /// Changes the default on all our devices; existing conversations keep their own timer.
    pub async fn set_default_disappearing(&self, seconds: i64) -> Result<DefaultDisappearing> {
        if !valid_default_timer(seconds) {
            return Err(CoreError::invalid("invalid default disappearing timer"));
        }
        let cur = self.default_timer_record();
        let r = DefaultTimerRecord { seconds, updated_at: now_ms().max(cur.updated_at + 1) };
        self.db.borrow().kv_set_json(KV_KEY, &r)?;
        self.vault_put_settings().await;
        let state = self.get_default_disappearing();
        self.emit("default_disappearing_changed", &state);
        Ok(state)
    }

    /// The setting from another of our devices (vault settings record): newest change wins.
    pub(super) fn apply_synced_default_timer(&self, remote: DefaultTimerRecord) -> Result<()> {
        if remote.updated_at <= self.default_timer_record().updated_at || !valid_default_timer(remote.seconds) {
            return Ok(());
        }
        let before = self.default_disappearing_seconds();
        self.db.borrow().kv_set_json(KV_KEY, &remote)?;
        if self.default_disappearing_seconds() != before {
            self.emit("default_disappearing_changed", &self.get_default_disappearing());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{Core, CoreConfig, EventSink};
    use crate::error::ErrorCode;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct Events(Mutex<Vec<String>>);
    impl EventSink for Events {
        fn emit(&self, e: &str, _: serde_json::Value) {
            self.0.lock().unwrap().push(e.to_string());
        }
    }

    #[test]
    fn only_the_offered_timers_are_valid() {
        for s in DEFAULT_TIMER_OPTIONS {
            assert!(valid_default_timer(s));
        }
        for s in [-1, 1, 60, 3599, 2 * 86400, 365 * 86400, i64::MAX] {
            assert!(!valid_default_timer(s), "{s}");
        }
        // Old or partial vault records mean "off".
        let r: DefaultTimerRecord = serde_json::from_str(r#"{"updated_at":5}"#).unwrap();
        assert_eq!(r, DefaultTimerRecord { seconds: 0, updated_at: 5 });
    }

    #[tokio::test]
    async fn set_validate_and_sync() {
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
            let st = i.get_default_disappearing();
            assert_eq!(st.seconds, 0, "off by default");
            assert_eq!(st.options, DEFAULT_TIMER_OPTIONS.to_vec());
            for bad in [-1, 1, 7200, 365 * 86400] {
                let e = i.set_default_disappearing(bad).await.unwrap_err();
                assert_eq!(e.code, ErrorCode::InvalidInput);
            }
            assert_eq!(i.default_disappearing_seconds(), 0, "a refused value changes nothing");
            assert_eq!(i.set_default_disappearing(3600).await?.seconds, 3600);

            // Another device: older change loses, invalid values are ignored, newer wins.
            let local = i.default_timer_record();
            i.apply_synced_default_timer(DefaultTimerRecord { seconds: 600, updated_at: local.updated_at - 1 })?;
            assert_eq!(i.default_disappearing_seconds(), 3600);
            i.apply_synced_default_timer(DefaultTimerRecord { seconds: 1234, updated_at: local.updated_at + 1 })?;
            assert_eq!(i.default_disappearing_seconds(), 3600);
            i.apply_synced_default_timer(DefaultTimerRecord { seconds: 86400, updated_at: local.updated_at + 2 })?;
            assert_eq!(i.default_disappearing_seconds(), 86400);
            i.set_default_disappearing(0).await?;
            assert!(i.default_timer_record().updated_at > local.updated_at + 2, "a local change is always the newest");
            // A damaged stored value counts as off.
            i.db.borrow().kv_set_json(KV_KEY, &DefaultTimerRecord { seconds: 5, updated_at: 1 })?;
            assert_eq!(i.default_disappearing_seconds(), 0);
            Ok(())
        })
        .await
        .unwrap();
        let changes = events.0.lock().unwrap().iter().filter(|e| *e == "default_disappearing_changed").count();
        assert_eq!(changes, 3, "both sets and the synced change");
    }
}
