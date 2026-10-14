//! Remote wipe of removed devices.
//!
//! When a device is removed from its account (unlinked from another device, replaced by an
//! account recovery, or the account deleted), it must delete its data even if it never unlocks
//! again: a lost or stolen phone starts locked (the local data needs the account password,
//! engine/local_lock.rs), so the engine never connects and never sees its token rejected.
//!
//! So every device keeps a random secret in `revocation.json`, outside the encrypted database,
//! and registers its SHA-256 with the server (PUT /v1/devices/current/revocation). When the
//! device is removed, the server keeps that hash; anyone holding the secret (only this device)
//! can ask POST /v1/devices/revocation-status, without logging in. The app asks for every
//! account saved on the device at startup, when it comes back to the foreground and every
//! few minutes ([`crate::profiles::Profiles::check_revoked`]), and deletes a removed
//! account's data without needing its password.
//!
//! Only an explicit server record says "removed": a device that is merely unknown to the server
//! (restored backup, network error, a server without this feature) is never wiped.

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::api::ApiClient;
use crate::crypto::{b64, random_bytes, sha256, unb64};
use crate::error::{Context, Result};

pub const FILE: &str = "revocation.json";

#[derive(Serialize, Deserialize, Clone)]
struct RevocationFile {
    v: u32,
    /// Server the device belongs to.
    server: String,
    /// The secret (32 random bytes, base64).
    secret: String,
    /// The server has the secret's hash (only then can "removed" be trusted).
    registered: bool,
}

fn read(dir: &Path) -> Option<RevocationFile> {
    let raw = std::fs::read(dir.join(FILE)).ok()?;
    serde_json::from_slice(&raw).ok()
}

fn write(dir: &Path, f: &RevocationFile) -> Result<()> {
    let tmp = dir.join(format!("{FILE}.tmp"));
    std::fs::write(&tmp, serde_json::to_vec(f)?).ctx("write revocation file")?;
    std::fs::rename(&tmp, dir.join(FILE)).ctx("save revocation file")?;
    Ok(())
}

/// Makes sure this device's revocation secret exists and its hash is registered with the
/// server (`api` carries the device token). Cheap when already done.
pub async fn ensure_registered(dir: &Path, api: &ApiClient) -> Result<()> {
    let mut f = match read(dir) {
        Some(f) if f.server == api.base => f,
        // New, or left over from another server: a fresh secret.
        _ => RevocationFile { v: 1, server: api.base.clone(), secret: b64(random_bytes::<32>()), registered: false },
    };
    if f.registered {
        return Ok(());
    }
    // Saved before the server knows it: a crash in between only means registering again.
    write(dir, &f)?;
    let hash = sha256(&unb64(&f.secret)?);
    let _: serde_json::Value = api.put("/v1/devices/current/revocation", &json!({ "hash": b64(hash) })).await?;
    f.registered = true;
    write(dir, &f)
}

/// Asks the server whether the device of the data in `dir` was removed. False when unknown
/// (no registered secret, the server does not say so, or it cannot be reached).
pub async fn was_removed(dir: &Path) -> bool {
    let Some(f) = read(dir).filter(|f| f.registered) else { return false };
    let api = ApiClient::new(&f.server, None);
    match api.post::<_, serde_json::Value>("/v1/devices/revocation-status", &json!({ "secret": f.secret })).await {
        Ok(v) => v.get("removed").and_then(|r| r.as_bool()) == Some(true),
        Err(e) => {
            log::debug!("revocation check: {e}");
            false
        }
    }
}

/// Deletes the revocation file (the data it belongs to was wiped).
pub fn forget(dir: &Path) {
    let _ = std::fs::remove_file(dir.join(FILE));
}
