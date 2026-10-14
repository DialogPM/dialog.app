//! Key transparency, client side (docs/KEY-TRANSPARENCY.md).
//!
//! The server keeps an append-only Merkle log of every account's identity key and signs its
//! tree heads with a key we pin on first contact (like the zkgroup parameters). Here we:
//!
//! 1. refuse a first contact whose identity key is not the one the log holds for the account
//!    (`kt_require`, called before a pre-key bundle is used);
//! 2. keep the latest verified tree head and accept a new one only with a valid consistency
//!    proof (the log is append-only: no rewritten history, no rollback);
//! 3. monitor the log for our own account: every entry is downloaded once, the tree is
//!    recomputed and compared with the signed head, and any entry with our account id and a
//!    key that is not ours raises an alert (the server published a fake key for us);
//! 4. gossip: every data message carries our latest head; a contact's head that is not
//!    consistent with ours (same size and another root, or no valid consistency proof) is a
//!    split view, raised as an alert with both signed heads as evidence;
//! 5. verify, in the background, every identity we pinned from an incoming first message.
//!
//! State lives in the kv table (per device, never synced).

use std::rc::Rc;
use std::time::Duration;

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::Inner;
use crate::crypto::{b64, unb64};
use crate::error::{CoreError, ErrorCode, Result};
use crate::keytrans::{self as kt, CompactRange, Entry, Failure, Gossip, TreeHead};
use crate::util::now_ms;

const KEY: &str = "kt_public_key";
const HEAD: &str = "kt_head";
const MONITOR: &str = "kt_monitor";
const ALERTS: &str = "kt_alerts";
const SELF_STATUS: &str = "kt_self";
const GOSSIP_SEEN: &str = "kt_gossip_seen";

const TICK: Duration = Duration::from_secs(60);
/// Head refresh + self-check interval.
const REFRESH_MS: i64 = 30 * 60 * 1000;
const PAGE: u64 = 1000;
/// Pages of the log downloaded per monitoring run (the rest at the next run).
const MAX_PAGES: usize = 50;
/// Background verifications of pinned identities per tick.
const SWEEP_BATCH: usize = 20;
const MAX_ALERTS: usize = 50;
const MAX_SEEN: usize = 64;

pub const VERIFIED: &str = "verified";
pub const PENDING: &str = "pending";
pub const ALERT: &str = "alert";
pub const UNAVAILABLE: &str = "unavailable";

/// A detected inconsistency (shown until dismissed).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct KtAlert {
    /// "split_view" | "fork" | "rollback" | "bad_signature" | "key_mismatch" |
    /// "own_key_mismatch" | "log_mismatch" | "log_key_changed"
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    pub detail: String,
    pub at: i64,
    /// Signed heads etc. proving the server misbehaved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<Value>,
}

/// Overall key transparency state (settings / banner).
#[derive(Debug, Clone, Serialize)]
pub struct KtOverview {
    /// The server publishes a key transparency log (and we pinned its key).
    pub enabled: bool,
    pub tree_size: Option<u64>,
    /// When the latest head was verified.
    pub checked_at: Option<i64>,
    /// Our own account in the log: verified | pending | alert | unavailable.
    pub own_status: String,
    /// Log entries checked by the self-monitor.
    pub monitored: u64,
    pub alerts: Vec<KtAlert>,
}

/// Key transparency state of one contact's identity key (safety number screen).
#[derive(Debug, Clone, Serialize)]
pub struct KtContactStatus {
    pub account_id: String,
    /// verified | pending | alert | unavailable
    pub status: String,
    pub tree_size: Option<u64>,
    pub checked_at: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ContactRecord {
    status: String,
    #[serde(default)]
    identity_key: Option<String>,
    #[serde(default)]
    index: Option<u64>,
    #[serde(default)]
    tree_size: Option<u64>,
    checked_at: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Monitor {
    range: CompactRange,
    #[serde(default)]
    own_index: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct SelfStatus {
    status: String,
    checked_at: i64,
}

#[derive(Deserialize)]
struct HeadResp {
    head: TreeHead,
    #[serde(default)]
    consistency: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct ProofResp {
    head: TreeHead,
    #[serde(default)]
    consistency: Option<Vec<String>>,
    entry: Entry,
    inclusion: Vec<String>,
}

#[derive(Deserialize)]
struct ConsistencyResp {
    proof: Vec<String>,
}

#[derive(Deserialize)]
struct EntriesResp {
    entries: Vec<Entry>,
}

fn contact_key(account_id: &str) -> String {
    format!("kt_contact:{account_id}")
}

fn failure_error(f: &Failure) -> CoreError {
    let msg = match f {
        Failure::BadSignature => "the key transparency tree head is not signed by the server's pinned key",
        Failure::Fork { .. } => "the server showed two inconsistent versions of the key transparency log",
        Failure::Rollback => "the server went back to an older key transparency log",
        Failure::BadProof => "the server's key transparency proof does not verify",
        Failure::KeyMismatch => "this key is not the one in the key transparency log",
    };
    CoreError::new(ErrorCode::Forbidden, msg)
}

impl Inner {
    fn kt_key(&self) -> Option<[u8; 32]> {
        let k: String = self.db.borrow().kv_get_json(KEY).ok().flatten()?;
        unb64(&k).ok()?.try_into().ok()
    }

    fn kt_head(&self) -> Option<TreeHead> {
        self.db.borrow().kv_get_json(HEAD).ok().flatten()
    }

    /// Our latest verified head, attached to outgoing data messages (gossip).
    pub(super) fn kt_gossip_head(&self) -> Option<TreeHead> {
        self.kt_key()?;
        self.kt_head()
    }

    fn kt_alerts(&self) -> Vec<KtAlert> {
        self.db.borrow().kv_get_json(ALERTS).ok().flatten().unwrap_or_default()
    }

    /// Records an alert (the same kind for the same account at most once a day) and tells the UI.
    fn kt_alert(&self, kind: &str, account_id: Option<&str>, detail: &str, evidence: Option<Value>) {
        let mut alerts = self.kt_alerts();
        let now = now_ms();
        if alerts.iter().any(|a| a.kind == kind && a.account_id.as_deref() == account_id && now - a.at < 24 * 3600 * 1000) {
            return;
        }
        log::error!("key transparency alert ({kind}){}: {detail}", account_id.map(|a| format!(" for {a}")).unwrap_or_default());
        alerts.push(KtAlert { kind: kind.into(), account_id: account_id.map(String::from), detail: detail.into(), at: now, evidence });
        if alerts.len() > MAX_ALERTS {
            alerts.drain(..alerts.len() - MAX_ALERTS);
        }
        let _ = self.db.borrow().kv_set_json(ALERTS, &alerts);
        self.emit("kt_alerts", &alerts);
    }

    /// Turns a refused view of the log into an alert and an error.
    fn kt_failed(&self, f: Failure, account_id: Option<&str>, from: Option<&str>) -> CoreError {
        let err = failure_error(&f);
        match &f {
            Failure::Fork { ours, theirs } => {
                let kind = if from.is_some() { "split_view" } else { "fork" };
                let evidence = json!({ "ours": ours, "theirs": theirs, "gossiped_by": from });
                self.kt_alert(kind, from.or(account_id), &err.message, Some(evidence));
            }
            Failure::Rollback => self.kt_alert("rollback", account_id, &err.message, None),
            Failure::BadSignature => self.kt_alert("bad_signature", account_id, &err.message, None),
            Failure::BadProof => self.kt_alert("log_mismatch", account_id, &err.message, None),
            Failure::KeyMismatch => self.kt_alert("key_mismatch", account_id, &err.message, None),
        }
        err
    }

    /// Pins the server's log key (each time the connection starts). A changed key is refused.
    pub(super) async fn kt_setup(&self) -> Result<bool> {
        let info: Value = self.session()?.api.get("/v1/info").await?;
        let Some(key) = info.get("kt_public_key").and_then(|v| v.as_str()).map(String::from) else {
            return Ok(self.kt_key().is_some());
        };
        if unb64(&key).ok().is_none_or(|k| k.len() != 32) {
            return Err(CoreError::internal("invalid key transparency key"));
        }
        let pinned: Option<String> = self.db.borrow().kv_get_json(KEY)?;
        match pinned {
            None => self.db.borrow().kv_set_json(KEY, &key)?,
            Some(p) if p == key => {}
            Some(_) => {
                self.kt_alert("log_key_changed", None, "the server's key transparency key changed: refusing it", None);
                return Err(CoreError::new(ErrorCode::Forbidden, "the server's key transparency key changed"));
            }
        }
        Ok(true)
    }

    async fn kt_get<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T> {
        self.session()?.api.get_anonymous(path).await.map_err(CoreError::from)
    }

    /// Accepts a head the server served (with its consistency proof from our stored size).
    fn kt_accept(&self, head: &TreeHead, consistency: Option<&[String]>) -> Result<TreeHead> {
        let key = self.kt_key().ok_or_else(|| CoreError::new(ErrorCode::NotFound, "no key transparency on this server"))?;
        let proof = match consistency {
            Some(p) => Some(kt::hashes_b64(p).ok_or_else(|| self.kt_failed(Failure::BadProof, None, None))?),
            None => None,
        };
        let ours = self.kt_head();
        let keep = kt::accept_head(&key, ours.as_ref(), head, proof.as_deref()).map_err(|f| self.kt_failed(f, None, None))?;
        if ours.as_ref() != Some(&keep) {
            self.db.borrow().kv_set_json(HEAD, &keep)?;
        }
        Ok(keep)
    }

    fn kt_since(&self) -> u64 {
        self.kt_head().map_or(0, |h| h.tree_size)
    }

    /// Fetches and verifies the latest head.
    pub async fn kt_update_head(&self) -> Result<TreeHead> {
        let r: HeadResp = self.kt_get(&format!("/v1/kt/head?since={}", self.kt_since())).await?;
        self.kt_accept(&r.head, r.consistency.as_deref())
    }

    /// Fetches `account_id`'s entry with proofs and checks it holds `identity_key` (when given).
    /// Outer error: network, not in the log, or a bad head (already alerted); inner error: the
    /// entry itself does not verify (left to the caller).
    async fn kt_fetch_entry(&self, account_id: &str, identity_key: Option<&[u8]>) -> Result<std::result::Result<Entry, Failure>> {
        let r: ProofResp = self.kt_get(&format!("/v1/kt/proof/{account_id}?since={}", self.kt_since())).await?;
        let head = r.head.clone();
        self.kt_accept(&r.head, r.consistency.as_deref())?;
        let Some(inclusion) = kt::hashes_b64(&r.inclusion) else { return Ok(Err(Failure::BadProof)) };
        Ok(kt::check_entry(&head, &r.entry, &inclusion, account_id, identity_key).map(|_| r.entry))
    }

    fn kt_pinned(&self, account_id: &str) -> Result<Option<Vec<u8>>> {
        Ok(self
            .db
            .borrow()
            .conn
            .query_row("SELECT identity_key FROM identities WHERE account_id = ?1", [account_id], |r| r.get(0))
            .optional()?)
    }

    fn kt_record(&self, account_id: &str) -> Option<ContactRecord> {
        self.db.borrow().kv_get_json(&contact_key(account_id)).ok().flatten()
    }

    fn kt_set_record(&self, account_id: &str, status: &str, identity_key: &[u8], index: Option<u64>) -> Result<()> {
        let r = ContactRecord {
            status: status.into(),
            identity_key: Some(b64(identity_key)),
            index,
            tree_size: self.kt_head().map(|h| h.tree_size),
            checked_at: now_ms(),
        };
        self.db.borrow().kv_set_json(&contact_key(account_id), &r)
    }

    /// Verifies that `identity_key` is `account_id`'s key in the log. A mismatch is recorded,
    /// raised as an alert and as an identity conflict (the conversation shows the warning).
    pub async fn kt_verify_contact(&self, account_id: &str, identity_key: &[u8]) -> Result<()> {
        match self.kt_fetch_entry(account_id, Some(identity_key)).await {
            Ok(Ok(e)) => self.kt_set_record(account_id, VERIFIED, identity_key, Some(e.index)),
            Ok(Err(f)) => {
                let e = self.kt_failed(f, Some(account_id), None);
                self.kt_set_record(account_id, ALERT, identity_key, None)?;
                self.identity_conflicts.borrow_mut().push(account_id.to_string());
                let _ = self.process_identity_conflicts();
                Err(e)
            }
            Err(e) if e.code == ErrorCode::NotFound => {
                Err(CoreError::network("the contact's key is not in the key transparency log yet").context(account_id))
            }
            Err(e) => Err(e),
        }
    }

    /// Before the first session with an account: its key must be the one in the log (when the
    /// server has a log). Already verified keys are not fetched again.
    pub(super) async fn kt_require(&self, account_id: &str, identity_key: &[u8]) -> Result<()> {
        if account_id == self.me()? {
            return Ok(());
        }
        if self.kt_key().is_none() && !self.kt_setup().await.unwrap_or(false) {
            return Ok(()); // a server without key transparency
        }
        if self.kt_pinned(account_id)?.is_some() {
            // Verified when it was pinned (or by the sweep); libsignal refuses any other key.
            return Ok(());
        }
        if let Some(r) = self.kt_record(account_id) {
            if r.status == VERIFIED && r.identity_key.as_deref() == Some(b64(identity_key).as_str()) {
                return Ok(());
            }
        }
        self.kt_verify_contact(account_id, identity_key).await
    }

    /// Self-check: our entry holds our key, and no entry of the log holds another key for us.
    async fn kt_monitor(&self) -> Result<()> {
        let s = self.session()?;
        let me = s.account.account_id.clone();
        let mine = s.keys.identity.identity_key().serialize().to_vec();
        let head = self.kt_update_head().await?;
        let mut status = match self.kt_fetch_entry(&me, Some(&mine)).await {
            Ok(Ok(_)) => VERIFIED,
            Ok(Err(Failure::KeyMismatch)) => {
                self.kt_alert("own_key_mismatch", Some(&me), "the log holds another identity key for your account", None);
                ALERT
            }
            Ok(Err(f)) => {
                self.kt_failed(f, Some(&me), None);
                ALERT
            }
            Err(e) if e.code == ErrorCode::NotFound => PENDING,
            Err(e) => return Err(e),
        };
        let mut m: Monitor = self.db.borrow().kv_get_json(MONITOR)?.unwrap_or_default();
        if m.range.size > head.tree_size {
            m = Monitor::default(); // cannot happen with an accepted head; start over
        }
        let root = head.root().ok_or_else(|| CoreError::internal("bad head"))?;
        let mut pages = 0;
        while m.range.size < head.tree_size && pages < MAX_PAGES {
            let (start, end) = (m.range.size, (m.range.size + PAGE).min(head.tree_size));
            let r: EntriesResp = self.kt_get(&format!("/v1/kt/entries?start={start}&end={end}")).await?;
            if r.entries.len() as u64 != end - start {
                return Err(self.kt_failed(Failure::BadProof, None, None));
            }
            for e in &r.entries {
                let Some(h) = e.hash().filter(|_| e.index == m.range.size) else {
                    return Err(self.kt_failed(Failure::BadProof, None, None));
                };
                if uuid::Uuid::parse_str(&e.account_id).ok() == uuid::Uuid::parse_str(&me).ok() {
                    if unb64(&e.identity_key).ok().as_deref() == Some(&mine[..]) {
                        m.own_index.get_or_insert(e.index);
                    } else {
                        self.kt_alert(
                            "own_key_mismatch",
                            Some(&me),
                            &format!("entry {} of the log holds another identity key for your account", e.index),
                            Some(json!({ "entry": e })),
                        );
                        status = ALERT;
                    }
                }
                m.range.push(h);
            }
            pages += 1;
        }
        if m.range.size == head.tree_size && m.range.root() != root {
            // The entries do not make the signed tree: start over next time, and say so.
            self.db.borrow().kv_set_json(MONITOR, &Monitor::default())?;
            return Err(self.kt_failed(Failure::BadProof, None, None));
        }
        self.db.borrow().kv_set_json(MONITOR, &m)?;
        if status != ALERT && self.kt_alerts().iter().any(|a| a.kind == "own_key_mismatch") {
            status = ALERT;
        }
        self.db.borrow().kv_set_json(SELF_STATUS, &SelfStatus { status: status.into(), checked_at: now_ms() })?;
        Ok(())
    }

    /// Verifies pinned identities that were not checked against the log yet (first messages
    /// received from new contacts).
    async fn kt_sweep(&self) {
        let Ok(me) = self.me() else { return };
        let pinned: Vec<(String, Vec<u8>)> = {
            let db = self.db.borrow();
            let Ok(mut st) = db.conn.prepare("SELECT account_id, identity_key FROM identities WHERE account_id != ?1") else { return };
            st.query_map(params![me], |r| Ok((r.get(0)?, r.get(1)?)))
                .map(|rows| rows.filter_map(|r| r.ok()).collect())
                .unwrap_or_default()
        };
        let mut done = 0;
        for (account, key) in pinned {
            if done >= SWEEP_BATCH {
                break;
            }
            let rec = self.kt_record(&account);
            let current = rec.as_ref().is_some_and(|r| r.identity_key.as_deref() == Some(b64(&key).as_str()));
            if current && rec.as_ref().is_some_and(|r| r.status == VERIFIED || r.status == ALERT) {
                continue;
            }
            done += 1;
            if let Err(e) = self.kt_verify_contact(&account, &key).await {
                log::warn!("key transparency check of {account}: {e}");
                if e.code == ErrorCode::Network {
                    break;
                }
            }
        }
    }

    /// A contact's message carried their latest head: compare views of the log.
    pub(super) fn kt_on_gossip(self: &Rc<Self>, from: &str, head: TreeHead) {
        let (me, from) = (self.clone(), from.to_string());
        tokio::task::spawn_local(async move {
            if let Err(e) = me.kt_gossip(&from, head).await {
                log::warn!("key transparency gossip from {from}: {e}");
            }
        });
    }

    async fn kt_gossip(&self, from: &str, theirs: TreeHead) -> Result<()> {
        let Some(key) = self.kt_key() else { return Ok(()) };
        if !theirs.verify(&key) {
            log::warn!("ignoring a tree head from {from} not signed by our server");
            return Ok(());
        }
        let tag = format!("{}:{}", theirs.tree_size, theirs.root_hash);
        let mut seen: Vec<String> = self.db.borrow().kv_get_json(GOSSIP_SEEN)?.unwrap_or_default();
        if seen.contains(&tag) {
            return Ok(());
        }
        let ours = match self.kt_head() {
            Some(h) => h,
            None => self.kt_update_head().await?,
        };
        let outcome = match kt::compare_gossip(&ours, &theirs) {
            Ok(Gossip::NeedProof(first, second)) => {
                match self.kt_get::<ConsistencyResp>(&format!("/v1/kt/consistency?first={first}&second={second}")).await {
                    Ok(r) => match kt::hashes_b64(&r.proof) {
                        Some(p) => kt::finish_gossip(&ours, &theirs, &p),
                        None => Err(Failure::Fork { ours: ours.clone(), theirs: theirs.clone() }),
                    },
                    // The server signed a head of this size and now cannot prove it.
                    Err(e) if e.code == ErrorCode::InvalidInput => Err(Failure::Fork { ours: ours.clone(), theirs: theirs.clone() }),
                    Err(e) => return Err(e),
                }
            }
            other => other,
        };
        match outcome {
            Ok(Gossip::Newer) => {
                // Proven to extend ours: it is now our latest head.
                self.db.borrow().kv_set_json(HEAD, &theirs)?;
            }
            Ok(_) => {}
            Err(f) => {
                return Err(self.kt_failed(f, None, Some(from)));
            }
        }
        seen.push(tag);
        if seen.len() > MAX_SEEN {
            seen.drain(..seen.len() - MAX_SEEN);
        }
        self.db.borrow().kv_set_json(GOSSIP_SEEN, &seen)?;
        Ok(())
    }

    /// Background task: pins the key, then keeps the head, self-check and contacts verified.
    pub(super) async fn kt_loop(self: Rc<Self>) {
        loop {
            match self.kt_setup().await {
                Ok(true) => break,
                Ok(false) => return, // the server has no key transparency log
                Err(e) if e.code == ErrorCode::Forbidden => return,
                Err(e) => log::debug!("key transparency setup: {e}"),
            }
            tokio::time::sleep(TICK).await;
        }
        let mut last_full = 0;
        loop {
            if now_ms() - last_full > REFRESH_MS {
                match self.kt_monitor().await {
                    Ok(()) => last_full = now_ms(),
                    Err(e) => log::warn!("key transparency self-check: {e}"),
                }
            }
            self.kt_sweep().await;
            tokio::time::sleep(TICK).await;
        }
    }

    pub fn kt_overview(&self) -> Result<KtOverview> {
        let head = self.kt_head();
        let enabled = self.kt_key().is_some();
        let own: Option<SelfStatus> = self.db.borrow().kv_get_json(SELF_STATUS)?;
        let monitored = self.db.borrow().kv_get_json::<Monitor>(MONITOR)?.map_or(0, |m| m.range.size);
        Ok(KtOverview {
            enabled,
            tree_size: head.as_ref().map(|h| h.tree_size),
            checked_at: head.map(|h| h.timestamp),
            own_status: if enabled { own.map(|o| o.status).unwrap_or_else(|| PENDING.into()) } else { UNAVAILABLE.into() },
            monitored,
            alerts: self.kt_alerts(),
        })
    }

    /// Status of a contact's key; checks it now when it was not verified yet.
    pub async fn kt_contact_status(&self, account_id: &str) -> Result<KtContactStatus> {
        let out = |r: Option<ContactRecord>, status: &str| KtContactStatus {
            account_id: account_id.to_string(),
            status: status.to_string(),
            tree_size: r.as_ref().and_then(|r| r.tree_size),
            checked_at: r.map(|r| r.checked_at),
        };
        if self.kt_key().is_none() {
            return Ok(out(None, UNAVAILABLE));
        }
        let Some(pinned) = self.kt_pinned(account_id)? else { return Ok(out(None, PENDING)) };
        let rec = self.kt_record(account_id).filter(|r| r.identity_key.as_deref() == Some(b64(&pinned).as_str()));
        if let Some(r) = rec.filter(|r| r.status == VERIFIED || r.status == ALERT) {
            let status = r.status.clone();
            return Ok(out(Some(r), &status));
        }
        let status = match self.kt_verify_contact(account_id, &pinned).await {
            Ok(()) => VERIFIED,
            Err(e) if e.code == ErrorCode::Forbidden => ALERT,
            Err(_) => PENDING,
        };
        Ok(out(self.kt_record(account_id), status))
    }

    /// Runs every check now (settings "Check now", tests).
    pub async fn kt_check_now(&self) -> Result<KtOverview> {
        if self.kt_key().is_none() && !self.kt_setup().await? {
            return self.kt_overview();
        }
        self.kt_monitor().await?;
        self.kt_sweep().await;
        self.kt_overview()
    }

    pub fn kt_dismiss_alerts(&self) -> Result<()> {
        self.db.borrow().kv_delete(ALERTS)?;
        self.emit("kt_alerts", &Vec::<KtAlert>::new());
        Ok(())
    }
}
