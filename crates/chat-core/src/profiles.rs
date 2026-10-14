//! Local profiles: several accounts on one device, each with its own data directory. Shared by
//! the desktop shell (app/src-tauri/src/profiles.rs) and the mobile bridge (mobile/rust).
//!
//! Layout under the base directory (the app's data directory):
//! - `profiles/<random id>/`: one account's data (database, lock file, media).
//! - the base directory itself: the account of versions before profiles, kept in place
//!   (registered as the profile "default" whose `dir` is "").
//! - `profiles.json`: the registry shown on the start screen. It never holds a password, a key
//!   or the account number: only a display label, the last 4 digits and the server host (all
//!   three left out for hidden profiles), the last use time, and a salted Argon2 hash of the
//!   number so a hidden profile can be opened by typing its full number.
//!
//! One profile is active per process. The app starts on a blank profile (a new, unregistered
//! directory whose data stays in memory) that shows the start screen; opening a profile starts a
//! second core on its directory, unlocks it with the password, then swaps it in and shuts the
//! previous core down (networking stopped, database closed). Only the active profile's core
//! runs, so notifications, calls and sync only ever come from the active account. A new
//! account (created, logged in or linked on the blank profile) joins the registry once its
//! local data is locked with the password. A profile whose data is wiped (log out, device
//! removed, account deleted, forgotten password) leaves the registry.
//!
//! Nothing here depends on a particular async runtime: blocking work (starting a core, the
//! Argon2 hash) runs on a short-lived thread.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Core, CoreConfig, CoreError, ErrorCode, EventSink};

pub const REGISTRY_FILE: &str = "profiles.json";
pub const PROFILES_DIR: &str = "profiles";
/// Id given to the account found directly in the base directory.
pub const LEGACY_ID: &str = "default";
/// Files of the local lock (engine/local_lock.rs): a profile holds an account on disk only
/// while one of them exists.
const LOCK_FILES: &[&str] = &["chat.db.lock", "chat.db.lock.tmp"];
/// Files a profile's data is made of (also the legacy profile inside the base directory).
const DATA_FILES: &[&str] = &["chat.db", "chat.db-wal", "chat.db-shm", "chat.db.lock", "chat.db.lock.tmp", "chat.db.lock.part", "db.key"];
const DATA_DIRS: &[&str] = &["media", "media-temp"];
/// Commands after which the active profile's registry entry may need to be created or updated
/// (the account was just set up, its data locked, or its name changed).
const REFRESH_AFTER: &[&str] = &[
    "register",
    "login",
    "recover",
    "login_wait_approval",
    "link_wait",
    "protect_local_data",
    "unlock",
    "set_profile",
    "confirm_mnemonic_saved",
];

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn has_lock(dir: &Path) -> bool {
    LOCK_FILES.iter().any(|f| dir.join(f).exists())
}

fn server_host(url: &str) -> Option<String> {
    let u = url::Url::parse(url).ok()?;
    let host = u.host_str()?;
    Some(match u.port() {
        Some(p) => format!("{host}:{p}"),
        None => host.to_string(),
    })
}

fn number_hash(number: &str, salt_hex: &str) -> Option<String> {
    let salt = hex::decode(salt_hex).ok()?;
    crate::crypto::profile_number_hash(number, &salt).ok().map(hex::encode)
}

/// Runs blocking work on its own thread and waits for it without blocking the caller's runtime.
async fn blocking<R: Send + 'static>(f: impl FnOnce() -> R + Send + 'static) -> Result<R, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    std::thread::Builder::new()
        .name("profiles".into())
        .spawn(move || {
            let _ = tx.send(f());
        })
        .map_err(|e| err("internal", &e.to_string()))?;
    rx.await.map_err(|e| err("internal", &e.to_string()))
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProfileEntry {
    pub id: String,
    /// Data directory relative to the base directory ("" = the base directory itself).
    pub dir: String,
    /// Profile name the user set (None: shown as "Account").
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub last4: Option<String>,
    #[serde(default)]
    pub server_host: Option<String>,
    /// Unix ms.
    #[serde(default)]
    pub last_used: i64,
    /// Not listed on the start screen ("Open another account on this device" finds it).
    #[serde(default)]
    pub hidden: bool,
    /// Hex salt and Argon2 hash of the account number (crypto::profile_number_hash).
    #[serde(default)]
    pub number_salt: Option<String>,
    #[serde(default)]
    pub number_hash: Option<String>,
}

impl ProfileEntry {
    /// Hidden profiles keep nothing that identifies the account.
    fn scrub_if_hidden(&mut self) {
        if self.hidden {
            self.label = None;
            self.last4 = None;
            self.server_host = None;
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Registry {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub profiles: Vec<ProfileEntry>,
}

/// A profile as listed on the start screen.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ProfileCard {
    pub id: String,
    pub label: Option<String>,
    pub last4: Option<String>,
    pub server_host: Option<String>,
    pub last_used: i64,
    /// Opened with its password since the app started: switching to it needs no password.
    pub unlocked: bool,
    /// The account in use now.
    pub active: bool,
}

impl Registry {
    /// Reads the registry; a missing or unreadable file gives an empty one (the profiles on disk
    /// are found again by [`Registry::reconcile`]).
    pub fn load(base: &Path) -> Self {
        match std::fs::read(base.join(REGISTRY_FILE)) {
            Ok(raw) => serde_json::from_slice(&raw).unwrap_or_else(|e| {
                log::warn!("unreadable profile list, rebuilding it: {e}");
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    /// Writes the registry atomically: a temporary file, flushed, then renamed over the old one.
    pub fn save(&self, base: &Path) -> std::io::Result<()> {
        use std::io::Write;
        let path = base.join(REGISTRY_FILE);
        let tmp = base.join(format!("{REGISTRY_FILE}.tmp"));
        let data = serde_json::to_vec_pretty(&Registry { version: 1, profiles: self.profiles.clone() })?;
        {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(&data)?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, &path)
    }

    pub fn get(&self, id: &str) -> Option<&ProfileEntry> {
        self.profiles.iter().find(|p| p.id == id)
    }

    fn get_mut(&mut self, id: &str) -> Option<&mut ProfileEntry> {
        self.profiles.iter_mut().find(|p| p.id == id)
    }

    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.profiles.len();
        self.profiles.retain(|p| p.id != id);
        self.profiles.len() != before
    }

    /// Matches the registry with the directories: entries whose data is gone are dropped, and
    /// locked data without an entry (the account of an older version in the base directory, or
    /// an entry lost in a crash) is added as an unnamed profile. Returns whether it changed.
    pub fn reconcile(&mut self, base: &Path) -> bool {
        let before = self.clone();
        self.profiles.retain(|p| is_safe_rel_dir(&p.dir) && has_lock(&base.join(&p.dir)));
        if has_lock(base) && !self.profiles.iter().any(|p| p.dir.is_empty()) {
            let id = if self.get(LEGACY_ID).is_none() { LEGACY_ID.to_string() } else { new_id() };
            self.profiles.push(ProfileEntry { id, dir: String::new(), ..Default::default() });
        }
        if let Ok(entries) = std::fs::read_dir(base.join(PROFILES_DIR)) {
            let mut found: Vec<String> = entries
                .filter_map(|e| e.ok())
                .filter(|e| e.path().is_dir() && has_lock(&e.path()))
                .filter_map(|e| e.file_name().to_str().map(str::to_string))
                .collect();
            found.sort();
            for name in found {
                let rel = format!("{PROFILES_DIR}/{name}");
                if !self.profiles.iter().any(|p| p.dir == rel) {
                    let id = if self.get(&name).is_none() { name } else { new_id() };
                    self.profiles.push(ProfileEntry { id, dir: rel, ..Default::default() });
                }
            }
        }
        *self != before
    }

    /// Profiles shown on the start screen, most recently used first.
    pub fn visible(&self) -> Vec<ProfileCard> {
        let mut list: Vec<ProfileCard> = self
            .profiles
            .iter()
            .filter(|p| !p.hidden)
            .map(|p| ProfileCard {
                id: p.id.clone(),
                label: p.label.clone(),
                last4: p.last4.clone(),
                server_host: p.server_host.clone(),
                last_used: p.last_used,
                unlocked: false,
                active: false,
            })
            .collect();
        list.sort_by(|a, b| b.last_used.cmp(&a.last_used));
        list
    }

    /// The profile of an account number (hidden or not), by its salted hash.
    pub fn find_by_number(&self, number: &str) -> Option<String> {
        self.profiles
            .iter()
            .find(|p| match (&p.number_salt, &p.number_hash) {
                (Some(salt), Some(hash)) => number_hash(number, salt).is_some_and(|h| crate::crypto::ct_eq(h.as_bytes(), hash.as_bytes())),
                _ => false,
            })
            .map(|p| p.id.clone())
    }
}

/// Registry paths stay inside the base directory.
fn is_safe_rel_dir(dir: &str) -> bool {
    dir.is_empty()
        || dir
            .strip_prefix(&format!("{PROFILES_DIR}/"))
            .is_some_and(|name| !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'))
}

fn new_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// Deletes a profile's local data. The base directory (legacy profile) keeps everything that
/// is not that account's data (the registry, the other profiles, the app's own files).
fn delete_profile_data(base: &Path, rel: &str) {
    if !is_safe_rel_dir(rel) {
        return;
    }
    if rel.is_empty() {
        for f in DATA_FILES {
            let _ = std::fs::remove_file(base.join(f));
        }
        for d in DATA_DIRS {
            let _ = std::fs::remove_dir_all(base.join(d));
        }
    } else if let Err(e) = std::fs::remove_dir_all(base.join(rel)) {
        log::warn!("could not delete profile {rel}: {e}");
    }
}

/// Removes directories under `profiles/` that hold no account (left by blank profiles that
/// were never set up, or whose data was wiped), except `keep` (the active one).
fn remove_orphans(base: &Path, registry: &Registry, keep: Option<&str>) {
    let Ok(entries) = std::fs::read_dir(base.join(PROFILES_DIR)) else { return };
    for e in entries.filter_map(|e| e.ok()) {
        let Some(name) = e.file_name().to_str().map(str::to_string) else { continue };
        let rel = format!("{PROFILES_DIR}/{name}");
        if Some(rel.as_str()) == keep || registry.profiles.iter().any(|p| p.dir == rel) || has_lock(&e.path()) {
            continue;
        }
        if e.path().is_dir() {
            delete_profile_data(base, &rel);
        }
    }
}

/// The registry file with its lock (never held across an await).
pub struct RegistryStore {
    base: PathBuf,
    reg: Mutex<Registry>,
}

impl RegistryStore {
    fn read<R>(&self, f: impl FnOnce(&Registry) -> R) -> R {
        f(&self.reg.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// Applies `f` and saves when it reports a change.
    fn update(&self, f: impl FnOnce(&mut Registry) -> bool) {
        let mut reg = self.reg.lock().unwrap_or_else(|e| e.into_inner());
        if f(&mut reg) {
            if let Err(e) = reg.save(&self.base) {
                log::error!("cannot save the profile list: {e}");
            }
        }
    }
}

/// Forwards one core's events to the UI while it is the active one, and drops its profile from
/// the registry when its data is wiped.
struct ProfileEvents {
    sink: Arc<dyn EventSink>,
    live: Arc<AtomicBool>,
    store: Arc<RegistryStore>,
    id: String,
}

impl EventSink for ProfileEvents {
    fn emit(&self, event: &str, payload: Value) {
        if event == "logged_out" {
            // The core deleted the data (the directory itself goes with the next cleanup).
            self.store.update(|r| r.remove(&self.id));
        }
        if self.live.load(Ordering::SeqCst) {
            self.sink.emit(event, payload);
        }
    }
}

#[derive(Clone)]
struct Active {
    core: Core,
    id: String,
    /// Relative to the base directory.
    rel: String,
    live: Arc<AtomicBool>,
}

pub struct Profiles {
    base: PathBuf,
    default_server_url: String,
    platform: String,
    sink: Arc<dyn EventSink>,
    store: Arc<RegistryStore>,
    active: RwLock<Active>,
    /// Serializes profile switches.
    switching: tokio::sync::Mutex<()>,
    /// Database keys of the profiles opened with their password since the app started, by
    /// profile id: switching back to one of them needs no password (quick switch). Only in
    /// memory, wiped when the app quits; a profile that leaves the registry loses its key.
    keys: Mutex<HashMap<String, zeroize::Zeroizing<[u8; 32]>>>,
}

/// Error strings in the `"<code>: <message>"` form the UIs parse (backend.ts / backend.dart).
fn err(code: &str, message: &str) -> String {
    format!("{code}: {message}")
}

impl Profiles {
    /// Loads the registry and starts a blank profile (the start screen).
    pub fn start(base: PathBuf, default_server_url: String, platform: String, sink: Arc<dyn EventSink>) -> crate::Result<Self> {
        std::fs::create_dir_all(&base)?;
        let mut reg = Registry::load(&base);
        if reg.reconcile(&base) {
            if let Err(e) = reg.save(&base) {
                log::error!("cannot save the profile list: {e}");
            }
        }
        remove_orphans(&base, &reg, None);
        let store = Arc::new(RegistryStore { base: base.clone(), reg: Mutex::new(reg) });
        let rel = format!("{PROFILES_DIR}/{}", new_id());
        let active = start_core(&base, &store, &sink, &default_server_url, &platform, new_id(), rel)?;
        active.live.store(true, Ordering::SeqCst);
        Ok(Self { base, default_server_url, platform, sink, store, active: RwLock::new(active), switching: Default::default(), keys: Default::default() })
    }

    fn current(&self) -> Active {
        self.active.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// The active profile's core.
    pub fn core(&self) -> Core {
        self.current().core
    }

    /// The active profile's data directory (served by the media protocol).
    pub fn data_dir(&self) -> PathBuf {
        self.base.join(self.current().rel)
    }

    pub fn list(&self) -> Vec<ProfileCard> {
        let active = self.current().id;
        let keys = self.keys.lock().unwrap_or_else(|e| e.into_inner());
        let mut list = self.store.read(Registry::visible);
        for card in &mut list {
            card.active = card.id == active;
            card.unlocked = card.active || keys.contains_key(&card.id);
        }
        list
    }

    /// Remembers the open database key of `active` for quick switching (when its data is
    /// locked with the password on disk and it is in the registry).
    async fn remember_key(&self, active: &Active) {
        if self.store.read(|r| r.get(&active.id).is_none()) {
            return;
        }
        if let Ok(Some(key)) = active.core.run(|i| async move { Ok(i.open_data_key()) }).await {
            self.keys.lock().unwrap_or_else(|e| e.into_inner()).insert(active.id.clone(), zeroize::Zeroizing::new(key));
        }
    }

    /// Quick switch to a profile opened with its password earlier in this run of the app, without
    /// asking for it again. "password_required" when it was not (or its key no longer works).
    pub async fn switch_to(&self, id: &str) -> Result<(), String> {
        let _guard = self.switching.lock().await;
        let Some(entry) = self.store.read(|r| r.get(id).cloned()) else {
            self.keys.lock().unwrap_or_else(|e| e.into_inner()).remove(id);
            return Err(err("not_found", "no such profile on this device"));
        };
        if self.current().rel == entry.dir {
            return Ok(());
        }
        let Some(key) = self.keys.lock().unwrap_or_else(|e| e.into_inner()).get(id).map(|k| **k) else {
            return Err(err("password_required", "open this account with its password first"));
        };
        let (base, store, sink) = (self.base.clone(), self.store.clone(), self.sink.clone());
        let (server, platform, entry_id, entry_dir) = (self.default_server_url.clone(), self.platform.clone(), entry.id.clone(), entry.dir.clone());
        let next = blocking(move || start_core(&base, &store, &sink, &server, &platform, entry_id, entry_dir)).await?.map_err(|e| e.to_string())?;
        if let Err(e) = next.core.run(move |i| async move { i.unlock_with_key(key).await }).await {
            next.core.shutdown().await;
            self.keys.lock().unwrap_or_else(|e| e.into_inner()).remove(id);
            return Err(err("password_required", &e.to_string()));
        }
        self.swap(next.clone()).await;
        self.refresh(&next, true).await;
        Ok(())
    }

    /// Runs a core command on the active profile, then updates its registry entry if needed.
    pub async fn dispatch(&self, name: &str, req: Value) -> Result<Value, String> {
        let active = self.current();
        let out = crate::rpc::dispatch(&active.core, name, req).await.map_err(|e| e.to_string())?;
        if REFRESH_AFTER.contains(&name) {
            self.refresh(&active, false).await;
        }
        Ok(out)
    }

    /// Runs a profile command (`profiles_list`, `profile_open`, ...) or else a core command on
    /// the active profile: the single entry point of a generic bridge (mobile).
    pub async fn invoke(&self, name: &str, req: Value) -> Result<Value, String> {
        fn parse<T: serde::de::DeserializeOwned>(req: Value) -> Result<T, String> {
            serde_json::from_value(req).map_err(|e| err("invalid_input", &e.to_string()))
        }
        let done = || Ok(Value::Null);
        match name {
            "profiles_list" => serde_json::to_value(self.list()).map_err(|e| err("internal", &e.to_string())),
            "profile_open" => {
                let r: OpenReq = parse(req)?;
                self.open(&r.id, &r.password).await.and_then(|_| done())
            }
            "profile_open_by_number" => {
                let r: OpenByNumberReq = parse(req)?;
                self.open_by_number(&r.number, &r.password).await.and_then(|_| done())
            }
            "profile_close" => self.close().await.and_then(|_| done()),
            "profile_forget" => {
                let r: IdReq = parse(req)?;
                self.forget(&r.id).await.and_then(|_| done())
            }
            "profile_current" => serde_json::to_value(self.current_settings()).map_err(|e| err("internal", &e.to_string())),
            "profile_set_visible" => {
                let r: VisibleReq = parse(req)?;
                self.set_visible(r.visible).await.and_then(|_| done())
            }
            "profile_switch" => {
                let r: IdReq = parse(req)?;
                self.switch_to(&r.id).await.and_then(|_| done())
            }
            "profiles_check_revoked" => Ok(serde_json::json!({ "wiped": self.check_revoked().await })),
            // Stateless: runs off the core thread (it takes ~1 s per minute of audio).
            "voice_mask" => {
                let r: VoiceMaskReq = parse(req)?;
                let out = voice_mask_base64(r.data_base64).await?;
                Ok(serde_json::json!({ "data_base64": out }))
            }
            _ => self.dispatch(name, req).await,
        }
    }

    /// Creates or updates the registry entry of `active` once its account is set up and its
    /// data locked with the password. `opened`: the user just opened it (last use time).
    async fn refresh(&self, active: &Active, opened: bool) {
        let info = active
            .core
            .run(|i| async move {
                let state = i.app_state();
                let name = i.get_profile().ok().map(|p| p.name.trim().to_string()).filter(|n| !n.is_empty());
                Ok((state, name))
            })
            .await;
        let Ok((state, name)) = info else { return };
        let Some(account) = state.account.filter(|_| state.status == "ready" && state.local_protected) else { return };
        let digits: String = account.number.chars().filter(|c| c.is_ascii_digit()).collect();
        let last4 = digits[digits.len().saturating_sub(4)..].to_string();
        // The number hash is computed once (an account's number never changes).
        let needs_hash = self.store.read(|r| r.get(&active.id).is_none_or(|e| e.number_hash.is_none()));
        let hash = if needs_hash {
            let salt = hex::encode(rand::random::<[u8; 16]>());
            let number = digits.clone();
            blocking(move || number_hash(&number, &salt).map(|h| (salt, h))).await.ok().flatten()
        } else {
            None
        };
        let host = server_host(&account.server_url);
        self.store.update(|r| {
            if r.get(&active.id).is_none() {
                r.profiles.push(ProfileEntry { id: active.id.clone(), dir: active.rel.clone(), last_used: now_ms(), ..Default::default() });
            }
            let Some(e) = r.get_mut(&active.id) else { return false };
            let before = e.clone();
            if let Some((salt, h)) = hash {
                e.number_salt = Some(salt);
                e.number_hash = Some(h);
            }
            e.label = name;
            e.last4 = Some(last4);
            e.server_host = host;
            if opened {
                e.last_used = now_ms();
            }
            e.scrub_if_hidden();
            *e != before
        });
        // Opened (or just set up) with its password: switching back to it needs none.
        self.remember_key(active).await;
    }

    /// Opens a profile with its password and makes it the active one. On a wrong password
    /// nothing changes.
    pub async fn open(&self, id: &str, password: &str) -> Result<(), String> {
        let _guard = self.switching.lock().await;
        let Some(entry) = self.store.read(|r| r.get(id).cloned()) else {
            return Err(err("not_found", "no such profile on this device"));
        };
        let current = self.current();
        if current.rel == entry.dir {
            // Already active: the password must still be right.
            let pw = password.to_string();
            current
                .core
                .run(|i| async move {
                    if i.locked.get() {
                        i.unlock(&pw).await
                    } else if i.verify_password(&pw).await? {
                        Ok(())
                    } else {
                        Err(CoreError::new(ErrorCode::InvalidPassword, "wrong password"))
                    }
                })
                .await
                .map_err(|e| e.to_string())?;
            self.refresh(&current, true).await;
            return Ok(());
        }
        let (base, store, sink) = (self.base.clone(), self.store.clone(), self.sink.clone());
        let (server, platform, entry_id, entry_dir) = (self.default_server_url.clone(), self.platform.clone(), entry.id.clone(), entry.dir.clone());
        let next = blocking(move || start_core(&base, &store, &sink, &server, &platform, entry_id, entry_dir)).await?.map_err(|e| e.to_string())?;
        let pw = password.to_string();
        if let Err(e) = next.core.run(|i| async move { i.unlock(&pw).await }).await {
            next.core.shutdown().await;
            return Err(e.to_string());
        }
        self.swap(next.clone()).await;
        self.refresh(&next, true).await;
        Ok(())
    }

    /// Opens the profile of a full account number (also hidden ones). A number with no profile
    /// here gets the same answer as a wrong password.
    pub async fn open_by_number(&self, number: &str, password: &str) -> Result<(), String> {
        let wrong = || err("invalid_password", "wrong account number or password");
        let Some(number) = crate::util::normalize_number(number) else { return Err(wrong()) };
        let snapshot = self.store.read(Registry::clone);
        let id = blocking(move || snapshot.find_by_number(&number)).await.ok().flatten().ok_or_else(wrong)?;
        self.open(&id, password).await
    }

    /// Back to the start screen on a new blank profile. The profile that was active stays on
    /// this device, locked (its core is shut down); nothing is logged out.
    pub async fn close(&self) -> Result<(), String> {
        let _guard = self.switching.lock().await;
        let current = self.current();
        let state = current.core.run(|i| async move { Ok(i.app_state()) }).await.map_err(|e| e.to_string())?;
        if state.account.is_some() && !state.local_protected {
            // Its data lives only in memory until locked: switching would lose it.
            return Err(err("invalid_input", "the local data of this account is not protected with the password yet"));
        }
        let (base, store, sink) = (self.base.clone(), self.store.clone(), self.sink.clone());
        let (server, platform) = (self.default_server_url.clone(), self.platform.clone());
        let next = blocking(move || start_core(&base, &store, &sink, &server, &platform, new_id(), format!("{PROFILES_DIR}/{}", new_id())))
            .await?
            .map_err(|e| e.to_string())?;
        self.swap(next).await;
        Ok(())
    }

    /// Makes `next` the active profile and stops the previous one.
    async fn swap(&self, next: Active) {
        next.live.store(true, Ordering::SeqCst);
        let old = std::mem::replace(&mut *self.active.write().unwrap_or_else(|e| e.into_inner()), next.clone());
        old.live.store(false, Ordering::SeqCst);
        teardown(&old.core).await;
        // A blank profile that was never set up (or was wiped) leaves no directory behind.
        let reg = self.store.read(Registry::clone);
        remove_orphans(&self.base, &reg, Some(&next.rel));
    }

    /// Deletes the local data of a profile that is not active (forgotten password).
    pub async fn forget(&self, id: &str) -> Result<(), String> {
        let _guard = self.switching.lock().await;
        let Some(entry) = self.store.read(|r| r.get(id).cloned()) else {
            return Err(err("not_found", "no such profile on this device"));
        };
        if entry.dir == self.current().rel {
            return Err(err("invalid_input", "the active profile is removed by logging out"));
        }
        delete_profile_data(&self.base, &entry.dir);
        self.store.update(|r| r.remove(id));
        self.keys.lock().unwrap_or_else(|e| e.into_inner()).remove(id);
        Ok(())
    }

    /// Asks the server, for every account saved on this device (locked or not), whether this
    /// device was removed from it, and deletes the data of those that were, without needing
    /// their password (crate::revocation). Returns the ids of the wiped profiles.
    pub async fn check_revoked(&self) -> Vec<String> {
        let current = self.current();
        let mut dirs: Vec<(String, String)> = self.store.read(|r| r.profiles.iter().map(|e| (e.id.clone(), e.dir.clone())).collect());
        if !dirs.iter().any(|(_, d)| *d == current.rel) {
            dirs.push((current.id.clone(), current.rel.clone()));
        }
        // The network part runs without holding the switch lock (opening a profile stays quick).
        let base = self.base.clone();
        let removed = blocking(move || {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().ok()?;
            Some(rt.block_on(async {
                let mut out = Vec::new();
                for (id, rel) in dirs {
                    if is_safe_rel_dir(&rel) && crate::revocation::was_removed(&base.join(&rel)).await {
                        out.push((id, rel));
                    }
                }
                out
            }))
        })
        .await
        .ok()
        .flatten()
        .unwrap_or_default();
        let mut wiped = Vec::new();
        if removed.is_empty() {
            return wiped;
        }
        let _guard = self.switching.lock().await;
        for (id, rel) in removed {
            let active = self.current();
            if rel == active.rel {
                // The running core wipes itself (its UI gets "logged_out", reason device_removed).
                let _ = active.core.run(|i| async move { i.wipe_local("device_removed") }).await;
            } else {
                delete_profile_data(&self.base, &rel);
                self.store.update(|r| r.remove(&id));
            }
            self.keys.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
            log::info!("profile {id}: this device was removed from the account, local data deleted");
            wiped.push(id);
        }
        wiped
    }

    /// Settings of the active profile: whether it is in the registry and listed.
    pub fn current_settings(&self) -> CurrentProfile {
        let id = self.current().id;
        self.store.read(|r| match r.get(&id) {
            Some(e) => CurrentProfile { registered: true, visible: !e.hidden },
            None => CurrentProfile { registered: false, visible: true },
        })
    }

    /// "Show this account on the start screen".
    pub async fn set_visible(&self, visible: bool) -> Result<(), String> {
        let active = self.current();
        if self.store.read(|r| r.get(&active.id).is_none()) {
            return Err(err("invalid_input", "this account is not saved on this device yet"));
        }
        self.store.update(|r| {
            let Some(e) = r.get_mut(&active.id) else { return false };
            e.hidden = !visible;
            e.scrub_if_hidden();
            true
        });
        if visible {
            // Put the label, last digits and host back.
            self.refresh(&active, false).await;
        }
        Ok(())
    }

    /// App exit: temporary-mode files of the active profile do not outlive the session.
    pub async fn exit(&self) {
        let core = self.core();
        let _ = core
            .run(|i| async move {
                if i.temporary_mode() {
                    i.wipe_temp_media();
                }
                Ok(())
            })
            .await;
    }
}

/// Starts a core on a profile directory (not yet forwarding its events to the UI).
fn start_core(
    base: &Path,
    store: &Arc<RegistryStore>,
    sink: &Arc<dyn EventSink>,
    default_server_url: &str,
    platform: &str,
    id: String,
    rel: String,
) -> crate::Result<Active> {
    let live = Arc::new(AtomicBool::new(false));
    let events = Arc::new(ProfileEvents { sink: sink.clone(), live: live.clone(), store: store.clone(), id: id.clone() });
    // No key is kept by the OS (keystore or file): the local data is written to disk only
    // once locked with the account password, so this per-run key never opens anything
    // stored (see engine/local_lock.rs).
    let db_key: [u8; 32] = rand::random();
    let core = Core::start(CoreConfig {
        data_dir: base.join(&rel),
        db_key,
        default_server_url: default_server_url.to_string(),
        platform: platform.to_string(),
        events,
    })?;
    Ok(Active { core, id, rel, live })
}

/// Stops a core that is no longer active: temporary-mode files are wiped, then networking and
/// the database are shut down.
async fn teardown(core: &Core) {
    let _ = core
        .run(|i| async move {
            if i.temporary_mode() {
                i.wipe_temp_media();
            }
            Ok(())
        })
        .await;
    core.shutdown().await;
}

#[derive(Debug, Serialize)]
pub struct CurrentProfile {
    pub registered: bool,
    pub visible: bool,
}

#[derive(Deserialize)]
pub struct OpenReq {
    pub id: String,
    pub password: String,
}

#[derive(Deserialize)]
pub struct OpenByNumberReq {
    pub number: String,
    pub password: String,
}

#[derive(Deserialize)]
pub struct IdReq {
    pub id: String,
}

#[derive(Deserialize)]
pub struct VisibleReq {
    pub visible: bool,
}

#[derive(Deserialize)]
pub struct VoiceMaskReq {
    pub data_base64: String,
}

/// Masks the voice in a base64 WAV recording on a worker thread (voice_mask.rs); returns the
/// masked WAV as base64.
pub async fn voice_mask_base64(data_base64: String) -> Result<String, String> {
    use base64::Engine as _;
    let (tx, rx) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        let b64 = base64::engine::general_purpose::STANDARD;
        let out = b64
            .decode(data_base64.trim())
            .map_err(|e| err("invalid_input", &e.to_string()))
            .and_then(|wav| crate::voice_mask::mask_wav(&wav).map_err(|e| err("invalid_input", &e)))
            .map(|masked| b64.encode(masked));
        let _ = tx.send(out);
    });
    rx.await.map_err(|_| err("internal", "voice masking stopped"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoEvents;
    impl EventSink for NoEvents {
        fn emit(&self, _: &str, _: Value) {}
    }

    fn temp_base(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("chatapp-profiles-{name}-{}-{}", std::process::id(), new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn entry(id: &str, dir: &str) -> ProfileEntry {
        ProfileEntry { id: id.into(), dir: dir.into(), ..Default::default() }
    }

    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(f)
    }

    #[test]
    fn registry_is_written_atomically() {
        let base = temp_base("atomic");
        let mut reg = Registry::default();
        reg.profiles.push(ProfileEntry { label: Some("Ana".into()), last4: Some("1234".into()), last_used: 5, ..entry("a", "profiles/a") });
        reg.save(&base).unwrap();
        assert!(!base.join(format!("{REGISTRY_FILE}.tmp")).exists(), "temporary file renamed");
        let loaded = Registry::load(&base);
        assert_eq!(loaded.profiles, reg.profiles);
        assert_eq!(loaded.version, 1);
        // A leftover temporary file from a crash never replaces the registry.
        std::fs::write(base.join(format!("{REGISTRY_FILE}.tmp")), b"{garbage").unwrap();
        assert_eq!(Registry::load(&base).profiles, reg.profiles);
        reg.profiles.push(entry("b", "profiles/b"));
        reg.save(&base).unwrap();
        assert_eq!(Registry::load(&base).profiles.len(), 2);
        // Nothing secret is stored.
        let raw = std::fs::read_to_string(base.join(REGISTRY_FILE)).unwrap();
        assert!(!raw.contains("password") && !raw.contains("key"));
        // A corrupt file reads as empty instead of failing the app start.
        std::fs::write(base.join(REGISTRY_FILE), b"not json").unwrap();
        assert!(Registry::load(&base).profiles.is_empty());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn hidden_profile_found_by_number_hash() {
        let number = "1234567812345678";
        let salt = hex::encode([7u8; 16]);
        let hash = number_hash(number, &salt).unwrap();
        let mut reg = Registry::default();
        let mut hidden = ProfileEntry {
            hidden: true,
            label: Some("Ana".into()),
            last4: Some("5678".into()),
            number_salt: Some(salt),
            number_hash: Some(hash.clone()),
            ..entry("h", "profiles/h")
        };
        hidden.scrub_if_hidden();
        assert_eq!((hidden.label.as_deref(), hidden.last4.as_deref()), (None, None));
        reg.profiles.push(hidden);
        reg.profiles.push(ProfileEntry { label: Some("Bob".into()), last_used: 3, ..entry("v", "profiles/v") });
        // Only the visible one is listed; the hidden one is found by its full number.
        assert_eq!(reg.visible().iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), ["v"]);
        assert_eq!(reg.find_by_number(number).as_deref(), Some("h"));
        assert_eq!(reg.find_by_number("1234567812345679"), None);
        // The registry holds the hash, never the number.
        let base = temp_base("hidden");
        reg.save(&base).unwrap();
        let raw = std::fs::read_to_string(base.join(REGISTRY_FILE)).unwrap();
        assert!(!raw.contains(number) && raw.contains(&hash));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn legacy_base_dir_is_a_profile() {
        let base = temp_base("legacy");
        // An account of an older version: data directly in the base directory, next to files
        // of the app itself (mobile keeps its preferences there).
        std::fs::write(base.join("chat.db"), b"db").unwrap();
        std::fs::write(base.join("chat.db.lock"), b"{}").unwrap();
        std::fs::write(base.join("prefs.json"), b"{}").unwrap();
        std::fs::create_dir_all(base.join("media")).unwrap();
        // A profile directory whose registry entry was lost, and an empty one (never set up).
        std::fs::create_dir_all(base.join("profiles/p1")).unwrap();
        std::fs::write(base.join("profiles/p1/chat.db.lock"), b"{}").unwrap();
        std::fs::create_dir_all(base.join("profiles/empty/media")).unwrap();
        let mut reg = Registry::default();
        reg.profiles.push(entry("gone", "profiles/gone"));
        reg.profiles.push(entry("evil", "../outside"));
        assert!(reg.reconcile(&base));
        let dirs: Vec<(&str, &str)> = reg.profiles.iter().map(|p| (p.id.as_str(), p.dir.as_str())).collect();
        assert_eq!(dirs, [(LEGACY_ID, ""), ("p1", "profiles/p1")]);
        assert!(!reg.reconcile(&base), "stable");
        remove_orphans(&base, &reg, None);
        assert!(!base.join("profiles/empty").exists());
        assert!(base.join("profiles/p1").exists());
        // The legacy data was not moved or touched.
        assert!(base.join("chat.db").exists() && base.join("media").exists());
        // Forgetting the legacy profile deletes only its own files from the base directory.
        reg.save(&base).unwrap();
        delete_profile_data(&base, "");
        assert!(!base.join("chat.db").exists() && !base.join("chat.db.lock").exists() && !base.join("media").exists());
        assert!(base.join(REGISTRY_FILE).exists() && base.join("profiles/p1/chat.db.lock").exists() && base.join("prefs.json").exists());
        assert!(reg.reconcile(&base));
        assert_eq!(reg.profiles.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), ["p1"]);
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Opening a profile swaps the core: wrong password leaves the start screen as it was, the
    /// right one unlocks the profile's data; closing goes back to a blank profile.
    #[test]
    fn open_and_close_profiles() {
        let base = temp_base("switch");
        let profiles = Profiles::start(base.clone(), String::new(), "test".into(), Arc::new(NoEvents)).unwrap();
        block_on(async {
            // Make the blank profile hold some locked data, as an account set up there would.
            let first = profiles.current();
            first.core.run(|i| async move { i.db.borrow().kv_set_json("marker", &42) }).await.unwrap();
            first.core.run(|i| async move { i.lock_with_password("pass word 1").await }).await.unwrap();
            profiles.store.update(|r| {
                r.profiles.push(ProfileEntry { label: Some("Ana".into()), ..entry(&first.id, &first.rel) });
                true
            });
            profiles.close().await.unwrap();
            let blank = profiles.current();
            assert_ne!(blank.rel, first.rel);
            assert!(first.core.run(|_| async move { Ok(()) }).await.is_err(), "previous core stopped");
            assert_eq!(profiles.list().len(), 1);

            let wrong = profiles.open(&first.id, "nope").await.unwrap_err();
            assert!(wrong.starts_with("invalid_password"), "{wrong}");
            assert_eq!(profiles.current().rel, blank.rel, "unchanged after a wrong password");
            profiles.open(&first.id, "pass word 1").await.unwrap();
            let opened = profiles.current();
            assert_eq!(opened.rel, first.rel);
            assert_eq!(profiles.data_dir(), base.join(&first.rel));
            let marker: Option<i32> = opened.core.run(|i| async move { i.db.borrow().kv_get_json("marker") }).await.unwrap();
            assert_eq!(marker, Some(42));
            // The blank profile left no directory behind.
            assert!(!base.join(&blank.rel).exists());
            assert!(profiles.forget(&first.id).await.is_err(), "the active profile is not forgotten");
            profiles.close().await.unwrap();
            profiles.forget(&first.id).await.unwrap();
            assert!(!base.join(&first.rel).exists());
            assert!(profiles.list().is_empty());
            profiles.core().shutdown().await;
        });
        let _ = std::fs::remove_dir_all(&base);
    }

    /// The generic entry point (mobile bridge) routes profile commands and core commands.
    #[test]
    fn invoke_routes_profile_and_core_commands() {
        let base = temp_base("invoke");
        let profiles = Profiles::start(base.clone(), String::new(), "test".into(), Arc::new(NoEvents)).unwrap();
        block_on(async {
            assert_eq!(profiles.invoke("profiles_list", Value::Null).await.unwrap(), serde_json::json!([]));
            let current = profiles.invoke("profile_current", Value::Null).await.unwrap();
            assert_eq!(current, serde_json::json!({ "registered": false, "visible": true }));
            let state = profiles.invoke("get_state", Value::Null).await.unwrap();
            assert_eq!(state["status"], "no_account");
            let e = profiles.invoke("profile_open", serde_json::json!({ "id": "nope", "password": "x" })).await.unwrap_err();
            assert!(e.starts_with("not_found"), "{e}");
            let e = profiles.invoke("profile_open", serde_json::json!({ "id": 1 })).await.unwrap_err();
            assert!(e.starts_with("invalid_input"), "{e}");
            let e = profiles.invoke("profile_open_by_number", serde_json::json!({ "number": "1234567812345678", "password": "x" })).await.unwrap_err();
            assert!(e.starts_with("invalid_password"), "{e}");
            // Voice masking works without an account (stateless).
            use base64::Engine as _;
            let b64 = base64::engine::general_purpose::STANDARD;
            let wav = crate::voice_mask::encode_wav(&vec![0.1; 8000], 16_000);
            let out = profiles.invoke("voice_mask", serde_json::json!({ "data_base64": b64.encode(&wav) })).await.unwrap();
            let masked = b64.decode(out["data_base64"].as_str().unwrap()).unwrap();
            assert_eq!(&masked[0..4], b"RIFF");
            let e = profiles.invoke("voice_mask", serde_json::json!({ "data_base64": b64.encode(b"not audio") })).await.unwrap_err();
            assert!(e.starts_with("invalid_input"), "{e}");
            profiles.core().shutdown().await;
        });
        let _ = std::fs::remove_dir_all(&base);
    }
}
