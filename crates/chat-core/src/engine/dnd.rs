//! Do not disturb: while it is on, incoming calls (one-to-one and group) do not ring on any of
//! our devices and become missed calls; the UIs also keep quiet about new messages. Nothing is
//! sent back to callers, so they only see an unanswered call and cannot learn about it.
//!
//! The setting is local and reaches our other devices through the vault settings record
//! (engine/vault.rs); the newest change wins.

use serde::{Deserialize, Serialize};

use super::Inner;
use crate::error::Result;
use crate::util::now_ms;

/// `until` of "until I turn it off".
pub const DND_FOREVER: i64 = i64::MAX;
/// Requested end times beyond this (year 33658, ms) mean "until I turn it off" (UIs whose
/// numbers cannot carry `i64::MAX` exactly send a large value instead).
const FOREVER_FROM: i64 = 1_000_000_000_000_000;
const KV_KEY: &str = "dnd";

/// Stored setting (also the vault copy).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct DndSetting {
    /// End of do not disturb (ms); 0 = off; [`DND_FOREVER`] = until turned off.
    pub until: i64,
    /// When it was changed (ms), to keep the newest change across devices.
    pub updated_at: i64,
}

/// Payload of `get_dnd` / `set_dnd` and of the `dnd_changed` event.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct DndState {
    /// 0 when off (or expired).
    pub until: i64,
}

/// Normalizes a requested end time: past values turn it off, very far ones mean forever.
fn normalize_until(until: i64, now: i64) -> i64 {
    if until >= FOREVER_FROM {
        DND_FOREVER
    } else if until <= now {
        0
    } else {
        until
    }
}

fn state_at(s: DndSetting, now: i64) -> DndState {
    DndState { until: if s.until > now { s.until } else { 0 } }
}

impl Inner {
    pub(super) fn dnd_setting(&self) -> DndSetting {
        self.db.borrow().kv_get_json(KV_KEY).ok().flatten().unwrap_or_default()
    }

    pub fn get_dnd(&self) -> DndState {
        state_at(self.dnd_setting(), now_ms())
    }

    /// Whether do not disturb is on right now (it ends on its own at `until`).
    pub fn dnd_active(&self) -> bool {
        self.get_dnd().until != 0
    }

    pub async fn set_dnd(&self, until: i64) -> Result<DndState> {
        let now = now_ms();
        let s = DndSetting { until: normalize_until(until, now), updated_at: now.max(self.dnd_setting().updated_at + 1) };
        self.db.borrow().kv_set_json(KV_KEY, &s)?;
        self.vault_put_settings().await;
        let state = state_at(s, now);
        self.emit("dnd_changed", &state);
        Ok(state)
    }

    /// The setting from another of our devices (vault settings record): newest change wins.
    pub(super) fn apply_synced_dnd(&self, remote: DndSetting) -> Result<()> {
        if remote.updated_at <= self.dnd_setting().updated_at {
            return Ok(());
        }
        let before = self.get_dnd();
        self.db.borrow().kv_set_json(KV_KEY, &remote)?;
        let after = self.get_dnd();
        if after != before {
            self.emit("dnd_changed", &after);
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
    fn until_is_normalized() {
        let now = 1_700_000_000_000;
        assert_eq!(normalize_until(0, now), 0);
        assert_eq!(normalize_until(-1, now), 0);
        assert_eq!(normalize_until(now - 5, now), 0, "a past end turns it off");
        assert_eq!(normalize_until(now + 3_600_000, now), now + 3_600_000);
        assert_eq!(normalize_until(i64::MAX, now), DND_FOREVER);
        assert_eq!(normalize_until(9_007_199_254_740_991, now), DND_FOREVER, "JS Number.MAX_SAFE_INTEGER");
    }

    #[test]
    fn expires_on_its_own() {
        let s = DndSetting { until: 1_000, updated_at: 1 };
        assert_eq!(state_at(s, 999).until, 1_000);
        assert_eq!(state_at(s, 1_000).until, 0, "over at `until`");
        assert_eq!(state_at(DndSetting { until: DND_FOREVER, updated_at: 1 }, i64::MAX - 1).until, DND_FOREVER);
        assert_eq!(state_at(DndSetting::default(), 0).until, 0);
    }

    #[tokio::test]
    async fn set_expire_and_sync() {
        let dir = tempfile::tempdir().unwrap();
        let events = Arc::new(Events::default());
        let core = Core::start(CoreConfig {
            data_dir: dir.path().to_path_buf(),
            db_key: [5u8; 32],
            default_server_url: String::new(),
            platform: "test".into(),
            events: events.clone(),
        })
        .unwrap();
        core.run(|i| async move {
            assert!(!i.dnd_active());
            let st = i.set_dnd(now_ms() + 300).await?;
            assert!(st.until > 0 && i.dnd_active());
            tokio::time::sleep(std::time::Duration::from_millis(400)).await;
            assert!(!i.dnd_active(), "expired");
            assert_eq!(i.get_dnd().until, 0);

            i.set_dnd(i64::MAX).await?;
            assert_eq!(i.get_dnd().until, DND_FOREVER);
            // An older change from another device loses; a newer one wins.
            let local = i.dnd_setting();
            i.apply_synced_dnd(DndSetting { until: 0, updated_at: local.updated_at - 1 })?;
            assert!(i.dnd_active());
            i.apply_synced_dnd(DndSetting { until: 0, updated_at: local.updated_at + 1 })?;
            assert!(!i.dnd_active());
            i.set_dnd(0).await?;
            assert!(i.dnd_setting().updated_at > local.updated_at + 1, "a local change is always the newest");
            Ok(())
        })
        .await
        .unwrap();
        let changes = events.0.lock().unwrap().iter().filter(|(e, _)| e == "dnd_changed").count();
        assert_eq!(changes, 4, "every set, and the synced change that turned it off");
    }
}
