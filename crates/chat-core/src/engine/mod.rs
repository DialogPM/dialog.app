//! The core engine.
//!
//! libsignal's store traits produce `!Send` futures, so all protocol and database work runs
//! on one dedicated thread with a single-threaded tokio runtime and a `LocalSet`. The rest of
//! the application talks to it through [`Core`], a cheap `Send + Sync` handle that ships
//! closures to that thread (see [`Core::run`]).

mod account;
mod attachments;
mod attest;
mod call_checks;
mod call_settings;
mod calls;
mod default_timer;
mod dnd;
mod group_calls;
mod temporary;
mod wipe;
mod zk;
mod contacts;
mod conversations;
mod edits;
mod integrity;
mod groups;
mod groups_v2;
pub mod group_bans;
pub mod invites;
pub mod keytrans;
mod link;
mod media;
mod local_lock;
mod marks;
mod messages;
mod net;
mod receive;
mod requests;
pub mod retention;
mod sealed;
pub mod security;
mod send;
mod vault;
mod view_once;
mod profile_key;
pub mod username_link;

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;

use libsignal_protocol::{DeviceId, ProtocolAddress};
use serde::Serialize;
use tokio::sync::{mpsc, oneshot, Notify};
use tokio::task::JoinHandle;

use crate::api::{ApiClient, RecipientDto};
use crate::crypto::{AccountKeys, AccountSecret};
use crate::db::Db;
use crate::error::{CoreError, ErrorCode, Result};
use crate::model::AccountInfo;
use crate::store::SignalStore;

pub use call_settings::{CallSettings, CallSettingsPatch};
pub use calls::{CallSignalRequest, RecordCallRequest};
pub use default_timer::DefaultDisappearing;
pub use dnd::{DndState, DND_FOREVER};
pub use group_calls::{GroupCallInfo, GroupCallJoinRequest, GroupCallSignalRequest, GroupCallStartRequest, MAX_PARTICIPANTS as MAX_GROUP_CALL_PARTICIPANTS};
pub use net::ConnState;

/// Receives events for the UI (implemented by the Tauri shell).
pub trait EventSink: Send + Sync + 'static {
    fn emit(&self, event: &str, payload: serde_json::Value);
}

/// Configuration supplied by the host application.
pub struct CoreConfig {
    /// Directory for the database and decrypted media.
    pub data_dir: PathBuf,
    /// 32-byte key of the local SQLCipher database (kept in the OS keystore by the host).
    pub db_key: [u8; 32],
    /// The built-in default server URL.
    pub default_server_url: String,
    /// Human-readable device name used for new logins when the UI does not provide one.
    pub platform: String,
    pub events: Arc<dyn EventSink>,
}

type Job = Box<dyn FnOnce(Rc<Inner>) -> Pin<Box<dyn Future<Output = ()>>> + Send>;

/// Thread-safe handle to the engine.
#[derive(Clone)]
pub struct Core {
    tx: mpsc::UnboundedSender<Job>,
    /// Completes when the engine thread has exited (database closed); see [`Core::shutdown`].
    stopped: Arc<std::sync::Mutex<Option<oneshot::Receiver<()>>>>,
}

impl Core {
    /// Starts the engine thread. Fails if the database cannot be opened.
    pub fn start(config: CoreConfig) -> Result<Self> {
        std::fs::create_dir_all(&config.data_dir)?;
        let (tx, mut rx) = mpsc::unbounded_channel::<Job>();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<()>>();
        let (stopped_tx, stopped_rx) = oneshot::channel::<()>();
        std::thread::Builder::new()
            .name("chat-core".into())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                    Ok(rt) => rt,
                    Err(e) => {
                        let _ = ready_tx.send(Err(CoreError::internal(e.to_string())));
                        return;
                    }
                };
                let local = tokio::task::LocalSet::new();
                local.block_on(&rt, async move {
                    let inner = match Inner::new(config) {
                        Ok(i) => Rc::new(i),
                        Err(e) => {
                            let _ = ready_tx.send(Err(e));
                            return;
                        }
                    };
                    let _ = ready_tx.send(Ok(()));
                    if inner.session().is_ok() {
                        inner.clone().start_background();
                    }
                    loop {
                        tokio::select! {
                            job = rx.recv() => match job {
                                Some(job) => {
                                    tokio::task::spawn_local(job(inner.clone()));
                                }
                                None => break,
                            },
                            _ = inner.shutdown.notified() => break,
                        }
                    }
                    inner.stop_background();
                });
                // Dropping the LocalSet drops the remaining tasks and with them the last
                // references to `Inner` (and its database connection), before the runtime.
                drop(local);
                // Bounded wait for blocking work still running (e.g. a password derivation).
                rt.shutdown_timeout(std::time::Duration::from_secs(5));
                let _ = stopped_tx.send(());
            })
            .map_err(|e| CoreError::internal(e.to_string()))?;
        ready_rx.recv().map_err(|_| CoreError::internal("engine thread died"))??;
        Ok(Self { tx, stopped: Arc::new(std::sync::Mutex::new(Some(stopped_rx))) })
    }

    /// Stops the engine: networking and background work end, pending requests fail, and the
    /// database is closed. Returns once the engine thread has exited (immediately for the
    /// other handles if several call it). Requests on any handle fail afterwards.
    pub async fn shutdown(&self) {
        let _ = self
            .run(|i| async move {
                i.stop_background();
                i.shutdown.notify_one();
                Ok(())
            })
            .await;
        let stopped = self.stopped.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(stopped) = stopped {
            let _ = stopped.await;
        }
    }

    /// Runs `f` on the engine thread and returns its result.
    pub async fn run<R, F, Fut>(&self, f: F) -> Result<R>
    where
        R: Send + 'static,
        F: FnOnce(Rc<Inner>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<R>> + 'static,
    {
        let (otx, orx) = oneshot::channel();
        let job: Job = Box::new(move |inner| {
            Box::pin(async move {
                let _ = otx.send(f(inner).await);
            })
        });
        self.tx.send(job).map_err(|_| CoreError::internal("engine stopped"))?;
        orx.await.map_err(|_| CoreError::internal("engine dropped the request"))?
    }
}

/// Logged-in account state.
pub struct Session {
    pub account: AccountInfo,
    pub api: ApiClient,
    pub keys: AccountKeys,
    pub registration_id: u32,
}

impl Session {
    pub fn local_address(&self) -> ProtocolAddress {
        ProtocolAddress::new(
            self.account.account_id.clone(),
            DeviceId::new(self.account.device_id as u8).expect("valid device id"),
        )
    }
}

/// Engine state. Lives on the engine thread only.
pub struct Inner {
    pub data_dir: PathBuf,
    /// Key from the host's keystore: opens the database while it is not password-locked.
    device_key: [u8; 32],
    /// Key the database is currently encrypted with.
    db_key: Cell<[u8; 32]>,
    /// Waiting for the password (see local_lock.rs); `db` is an empty in-memory database.
    pub locked: Cell<bool>,
    pub default_server_url: String,
    pub platform: String,
    events: Arc<dyn EventSink>,
    pub db: Rc<RefCell<Db>>,
    session: RefCell<Option<Rc<Session>>>,
    pub identity_conflicts: Rc<RefCell<Vec<String>>>,
    /// Frames to send on the WebSocket (None while disconnected).
    pub ws_out: RefCell<Option<mpsc::UnboundedSender<String>>>,
    pub conn_state: Cell<ConnState>,
    pub outbox_notify: Rc<Notify>,
    /// Wakes the expiry loop when a disappearing message is stored.
    pub expiry_notify: Rc<Notify>,
    pub vault_notify: Rc<Notify>,
    /// A push said messages are waiting: reconnect now, or check that the socket still lives.
    pub net_wake: Rc<Notify>,
    /// Mobile app in the background (engine/net.rs): the socket closes at this deadline and opens
    /// again only for pushes (until the next deadline) or when the app comes back. None: foreground.
    pub bg_deadline: Cell<Option<tokio::time::Instant>>,
    /// The user is at this device (engine/net.rs `set_presence_active`); when not, the time
    /// they went away (ms). The account shows online only while one of its devices is active.
    pub away_since: Cell<Option<i64>>,
    /// The foreground/background state changed.
    pub bg_changed: Rc<Notify>,
    pub presence_watch: RefCell<HashSet<String>>,
    /// group_id -> recipients (all devices of all members except ours), refreshed on 409/410.
    pub group_devices: RefCell<HashMap<String, Vec<RecipientDto>>>,
    background: RefCell<Vec<JoinHandle<()>>>,
    /// Serializes Signal-session-mutating send operations.
    pub send_lock: Rc<tokio::sync::Mutex<()>>,
    /// Set after a send to our own account succeeded with no other devices.
    pub no_other_devices: Cell<bool>,
    /// Device linking in progress on this device (either side).
    pub link: RefCell<Option<link::LinkSession>>,
    /// Recent call offers per caller: (call id, time), for the limit in calls.rs.
    pub call_offers: RefCell<HashMap<String, Vec<(String, i64)>>>,
    /// Group calls in progress, by group id (group_calls.rs).
    pub group_calls: RefCell<HashMap<String, group_calls::GroupCall>>,
    /// A password login of this device waiting for approval (security.rs).
    pub pending_login: RefCell<Option<security::PendingLogin>>,
    /// Ends the engine loop ([`Core::shutdown`]).
    shutdown: Notify,
}

impl Inner {
    fn new(config: CoreConfig) -> Result<Self> {
        let db_path = config.data_dir.join("chat.db");
        local_lock::finish_interrupted_lock(&config.data_dir, &config.db_key);
        let locked = local_lock::is_locked_on_disk(&config.data_dir);
        // On disk only once locked with the account password (local_lock.rs); until then the
        // data stays in memory. An older unprotected database that the given key cannot open
        // (it was keyed by the OS keystore, which is no longer used) is discarded.
        let db = if locked {
            Db::open_in_memory()?
        } else if db_path.exists() {
            match Db::open(&db_path, &config.db_key) {
                Ok(db) => db,
                Err(e) => {
                    log::warn!("discarding an unprotected local database that cannot be opened: {e}");
                    for f in ["chat.db", "chat.db-wal", "chat.db-shm", "db.key"] {
                        let _ = std::fs::remove_file(config.data_dir.join(f));
                    }
                    Db::open_in_memory()?
                }
            }
        } else {
            Db::open_in_memory()?
        };
        std::fs::create_dir_all(config.data_dir.join("media"))?;
        let inner = Self {
            data_dir: config.data_dir,
            device_key: config.db_key,
            db_key: Cell::new(config.db_key),
            locked: Cell::new(locked),
            default_server_url: config.default_server_url,
            platform: config.platform,
            events: config.events,
            db: Rc::new(RefCell::new(db)),
            session: RefCell::new(None),
            identity_conflicts: Rc::new(RefCell::new(Vec::new())),
            ws_out: RefCell::new(None),
            conn_state: Cell::new(ConnState::Offline),
            outbox_notify: Rc::new(Notify::new()),
            expiry_notify: Rc::new(Notify::new()),
            vault_notify: Rc::new(Notify::new()),
            net_wake: Rc::new(Notify::new()),
            bg_deadline: Cell::new(None),
            away_since: Cell::new(None),
            bg_changed: Rc::new(Notify::new()),
            presence_watch: RefCell::new(HashSet::new()),
            group_devices: RefCell::new(HashMap::new()),
            background: RefCell::new(Vec::new()),
            send_lock: Rc::new(tokio::sync::Mutex::new(())),
            no_other_devices: Cell::new(false),
            link: RefCell::new(None),
            call_offers: RefCell::new(HashMap::new()),
            group_calls: RefCell::new(HashMap::new()),
            pending_login: RefCell::new(None),
            shutdown: Notify::new(),
        };
        if !locked {
            inner.load_session()?;
            inner.start_temporary_mode()?;
        }
        Ok(inner)
    }

    pub fn media_dir(&self) -> PathBuf {
        self.data_dir.join("media")
    }

    pub fn emit<T: Serialize>(&self, event: &str, payload: &T) {
        match serde_json::to_value(payload) {
            Ok(v) => self.events.emit(event, v),
            Err(e) => log::error!("cannot serialize event {event}: {e}"),
        }
    }

    pub fn session(&self) -> Result<Rc<Session>> {
        self.session
            .borrow()
            .clone()
            .ok_or_else(|| CoreError::new(ErrorCode::NotLoggedIn, "not logged in"))
    }

    pub fn me(&self) -> Result<String> {
        Ok(self.session()?.account.account_id.clone())
    }

    pub fn signal_store(&self) -> Result<SignalStore> {
        let s = self.session()?;
        Ok(SignalStore {
            db: self.db.clone(),
            identity: s.keys.identity,
            registration_id: s.registration_id,
            account_id: s.account.account_id.clone(),
            identity_conflicts: self.identity_conflicts.clone(),
        })
    }

    /// Restores the session persisted in the database, if any.
    fn load_session(&self) -> Result<()> {
        let db = self.db.borrow();
        let account: Option<AccountInfo> = db.kv_get_json("account")?;
        let token: Option<String> = db.kv_get_json("token")?;
        let secret = db.kv_get("account_secret")?;
        let reg: Option<u32> = db.kv_get_json("registration_id")?;
        if let (Some(account), Some(token), Some(secret), Some(reg)) = (account, token, secret, reg) {
            let secret = AccountSecret(crate::crypto::key32(&secret)?);
            let keys = AccountKeys::derive(&secret)?;
            let api = ApiClient::new(&account.server_url, Some(token));
            *self.session.borrow_mut() = Some(Rc::new(Session { account, api, keys, registration_id: reg }));
        }
        Ok(())
    }

    /// Persists a new session and makes it current.
    fn set_session(&self, account: AccountInfo, token: String, secret: &AccountSecret, reg: u32) -> Result<()> {
        {
            let db = self.db.borrow();
            db.kv_set_json("account", &account)?;
            db.kv_set_json("token", &token)?;
            db.kv_set("account_secret", &secret.0)?;
            db.kv_set_json("registration_id", &reg)?;
        }
        let keys = AccountKeys::derive(secret)?;
        let api = ApiClient::new(&account.server_url, Some(token));
        *self.session.borrow_mut() = Some(Rc::new(Session { account, api, keys, registration_id: reg }));
        Ok(())
    }

    /// Starts the WebSocket loop, the outbox worker and periodic maintenance.
    pub fn start_background(self: Rc<Self>) {
        self.stop_background();
        let mut tasks = Vec::new();
        tasks.push(tokio::task::spawn_local(self.clone().connection_loop()));
        tasks.push(tokio::task::spawn_local(self.clone().outbox_loop()));
        tasks.push(tokio::task::spawn_local(self.clone().expiry_loop()));
        tasks.push(tokio::task::spawn_local(self.clone().maintenance_loop()));
        tasks.push(tokio::task::spawn_local(self.clone().vault_loop()));
        tasks.push(tokio::task::spawn_local(self.clone().media_loop()));
        tasks.push(tokio::task::spawn_local(self.clone().kt_loop()));
        tasks.push(tokio::task::spawn_local(self.clone().view_once_loop()));
        self.resume_profile_key_rotation();
        tasks.push(tokio::task::spawn_local(self.clone().group_call_loop()));
        tasks.push(tokio::task::spawn_local(self.clone().revocation_setup()));
        let me = self.clone();
        tasks.push(tokio::task::spawn_local(async move {
            if let Err(e) = me.zk_setup().await {
                log::warn!("zkgroup setup: {e}");
            }
            if let Err(e) = me.sealed_setup().await {
                log::warn!("sealed sender setup: {e}");
            }
        }));
        *self.background.borrow_mut() = tasks;
    }

    /// Registers this device's remote-wipe secret with the server (crate::revocation), retrying
    /// while offline.
    async fn revocation_setup(self: Rc<Self>) {
        let mut wait = std::time::Duration::from_secs(30);
        loop {
            let api = match self.session() {
                Ok(s) => s.api.clone(),
                Err(_) => return,
            };
            match crate::revocation::ensure_registered(&self.data_dir, &api).await {
                Ok(()) => return,
                Err(e) => log::warn!("remote wipe setup: {e}"),
            }
            tokio::time::sleep(wait).await;
            wait = (wait * 2).min(std::time::Duration::from_secs(3600));
        }
    }

    pub fn stop_background(&self) {
        for t in self.background.borrow_mut().drain(..) {
            t.abort();
        }
        *self.ws_out.borrow_mut() = None;
    }

    /// Wipes all local data (after logout / device removal / account deletion).
    pub fn wipe_local(&self, reason: &str) -> Result<()> {
        self.stop_background();
        *self.session.borrow_mut() = None;
        self.group_devices.borrow_mut().clear();
        self.presence_watch.borrow_mut().clear();
        self.group_calls.borrow_mut().clear();
        {
            // Swap in a throwaway DB so the file can be deleted.
            let mut db = self.db.borrow_mut();
            *db = Db::open_in_memory()?;
        }
        for f in ["chat.db", "chat.db-wal", "chat.db-shm"] {
            let _ = std::fs::remove_file(self.data_dir.join(f));
        }
        let _ = std::fs::remove_dir_all(self.media_dir());
        let _ = std::fs::remove_dir_all(self.data_dir.join("media-temp"));
        crate::revocation::forget(&self.data_dir);
        std::fs::create_dir_all(self.media_dir())?;
        self.remove_lock_files();
        // Back to memory until the next login locks the data with the password.
        *self.db.borrow_mut() = Db::open_in_memory()?;
        self.set_conn_state(ConnState::Offline);
        self.emit("logged_out", &serde_json::json!({ "reason": reason }));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoEvents;
    impl EventSink for NoEvents {
        fn emit(&self, _: &str, _: serde_json::Value) {}
    }

    /// After shutdown the engine thread is gone: requests fail instead of hanging.
    #[test]
    fn shutdown_stops_the_engine() {
        let dir = tempfile::tempdir().unwrap();
        let core = Core::start(CoreConfig {
            data_dir: dir.path().to_path_buf(),
            db_key: [5u8; 32],
            default_server_url: String::new(),
            platform: "test".into(),
            events: Arc::new(NoEvents),
        })
        .unwrap();
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async {
            assert!(core.run(|i| async move { Ok(i.locked.get()) }).await.is_ok());
            let other = core.clone();
            core.shutdown().await;
            other.shutdown().await;
            assert!(other.run(|_| async move { Ok(()) }).await.is_err());
        });
    }
}
