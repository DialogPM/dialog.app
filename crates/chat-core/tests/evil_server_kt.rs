//! Key transparency against a compromised server (docs/KEY-TRANSPARENCY.md). The tests play the
//! attacker through direct access to the server's MySQL database:
//!
//! 1. the server hands out a fake identity key on first contact (no log entry for it): refused;
//! 2. the server also forges a log entry for the fake key: the first contact is fooled, but the
//!    account owner's self-monitoring finds the foreign entry;
//! 3. the server rewrites its log and shows different versions to different users (split view):
//!    a contact's gossiped tree head exposes it.
//!
//! `CHATAPP_E2E_SERVER=http://127.0.0.1:8094 CHATAPP_E2E_MYSQL="-h127.0.0.1 -P3307 -uchatapp -p... chatapp_kt"
//!  cargo test -p chat-core --test evil_server_kt -- --nocapture`

use std::sync::{Arc, Mutex};
use std::time::Duration;

use chat_core::model::*;
use chat_core::{Core, CoreConfig, EventSink};
use serde_json::Value;

/// The scenarios rewrite the shared log: one at a time.
static SERIAL: Mutex<()> = Mutex::new(());

#[derive(Default)]
struct Events(Mutex<Vec<(String, Value)>>);

impl EventSink for Events {
    fn emit(&self, event: &str, payload: Value) {
        self.0.lock().unwrap().push((event.to_string(), payload));
    }
}

struct Client {
    name: &'static str,
    core: Core,
    events: Arc<Events>,
    _dir: tempfile::TempDir,
}

impl Client {
    fn new(name: &'static str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let events = Arc::new(Events::default());
        let core = Core::start(CoreConfig {
            data_dir: dir.path().to_path_buf(),
            db_key: [7u8; 32],
            default_server_url: server(),
            platform: "cli".into(),
            events: events.clone(),
        })
        .unwrap();
        Self { name, core, events, _dir: dir }
    }

    async fn wait_event(&self, what: &str, pred: impl Fn(&str, &Value) -> bool) -> Value {
        for _ in 0..300 {
            if let Some((_, v)) = self.events.0.lock().unwrap().iter().find(|(n, v)| pred(n, v)) {
                return v.clone();
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("{}: timed out waiting for {what}", self.name);
    }

    async fn wait_message(&self, text: &str) -> Value {
        let t = text.to_string();
        self.wait_event(&format!("message '{text}'"), move |n, v| n == "message" && v["message"]["text"].as_str() == Some(t.as_str()))
            .await["message"]
            .clone()
    }

    async fn wait_status(&self, id: &str, status: &str) {
        let (id, st) = (id.to_string(), status.to_string());
        self.wait_event(&format!("status {status}"), move |n, v| {
            n == "message" && v["message"]["id"].as_str() == Some(id.as_str()) && v["message"]["status"].as_str() == Some(st.as_str())
        })
        .await;
    }

    fn saw_message(&self, text: &str) -> bool {
        self.events.0.lock().unwrap().iter().any(|(n, v)| n == "message" && v["message"]["text"].as_str() == Some(text))
    }

    /// Waits for a key transparency alert of this kind.
    async fn wait_kt_alert(&self, kind: &str) -> Value {
        let k = kind.to_string();
        self.wait_event(&format!("kt alert {kind}"), move |n, v| {
            n == "kt_alerts" && v.as_array().is_some_and(|a| a.iter().any(|x| x["kind"].as_str() == Some(k.as_str())))
        })
        .await
    }

    fn kt_alerts(&self) -> Vec<String> {
        self.events
            .0
            .lock()
            .unwrap()
            .iter()
            .filter(|(n, _)| n == "kt_alerts")
            .flat_map(|(_, v)| v.as_array().cloned().unwrap_or_default())
            .filter_map(|a| a["kind"].as_str().map(String::from))
            .collect()
    }

    async fn wait_online(&self) {
        self.wait_event("online", |n, v| n == "connection" && v.as_str() == Some("online")).await;
    }

    async fn register(&self) -> AccountInfo {
        let u = server();
        let r = self.core.run(move |i| async move { i.register(&u, "correct horse battery", "test device").await }).await.unwrap();
        self.wait_online().await;
        r.account
    }

    async fn conversation_with(&self, number: &str) -> (String, String) {
        let n = number.to_string();
        let contact = self.core.run(move |i| async move { i.add_contact_by_number(&n, None).await }).await.unwrap();
        let id = contact.account_id.clone();
        let conv = self.core.run(move |i| async move { i.open_direct_conversation(&id).await }).await.unwrap();
        (contact.account_id, conv.id)
    }

    async fn kt_contact(&self, account_id: &str) -> String {
        let a = account_id.to_string();
        let st = self.core.run(move |i| async move { i.kt_contact_status(&a).await }).await.unwrap();
        st.status
    }

    async fn kt_check(&self) -> chat_core::engine::keytrans::KtOverview {
        self.core.run(|i| async move { i.kt_check_now().await }).await.unwrap()
    }
}

fn server() -> String {
    std::env::var("CHATAPP_E2E_SERVER").unwrap_or_default()
}

fn enabled() -> bool {
    if server().is_empty() || std::env::var("CHATAPP_E2E_MYSQL").is_err() {
        eprintln!("CHATAPP_E2E_SERVER / CHATAPP_E2E_MYSQL not set, skipping");
        return false;
    }
    let _ = env_logger::builder().is_test(true).try_init();
    true
}

async fn send_text(c: &Client, conv: &str, text: &str) -> Message {
    let req: SendMessageRequest = serde_json::from_value(serde_json::json!({ "conversation_id": conv, "text": text })).unwrap();
    c.core.run(move |i| async move { i.send_message(req).await }).await.unwrap()
}

/// Runs SQL on the server's database (the attacker's position).
fn sql(query: &str) -> String {
    let args = std::env::var("CHATAPP_E2E_MYSQL").expect("CHATAPP_E2E_MYSQL");
    let out = std::process::Command::new("mysql")
        .args(args.split_whitespace())
        .args(["-N", "-B", "-e", query])
        .output()
        .expect("run mysql");
    assert!(out.status.success(), "mysql: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// The server now advertises Mallory's identity and keys for Bob's device (a complete,
/// validly signed bundle). Returns the SQL that undoes it.
fn swap_bundle(b: &AccountInfo, m: &AccountInfo) -> String {
    let (b, bd, m, md) = (&b.account_id, b.device_id, &m.account_id, m.device_id);
    let original = sql(&format!("SELECT HEX(identity_key) FROM accounts WHERE id = '{b}'"));
    let backup = format!("evil_kt_{}", &b[..8]);
    sql(&format!(
        "CREATE TABLE {backup}_s AS SELECT * FROM device_signed_keys WHERE account_id = '{b}' AND device_id = {bd}; \
         CREATE TABLE {backup}_o AS SELECT * FROM device_one_time_keys WHERE account_id = '{b}' AND device_id = {bd}; \
         UPDATE accounts SET identity_key = (SELECT k FROM (SELECT identity_key AS k FROM accounts WHERE id = '{m}') t) WHERE id = '{b}'; \
         DELETE FROM device_signed_keys WHERE account_id = '{b}' AND device_id = {bd}; \
         DELETE FROM device_one_time_keys WHERE account_id = '{b}' AND device_id = {bd}; \
         INSERT INTO device_signed_keys SELECT '{b}', {bd}, kind, key_id, public_key, signature FROM device_signed_keys WHERE account_id = '{m}' AND device_id = {md}; \
         INSERT INTO device_one_time_keys SELECT '{b}', {bd}, kind, key_id, public_key, signature FROM device_one_time_keys WHERE account_id = '{m}' AND device_id = {md};"
    ));
    format!(
        "UPDATE accounts SET identity_key = UNHEX('{original}') WHERE id = '{b}'; \
         DELETE FROM device_signed_keys WHERE account_id = '{b}' AND device_id = {bd}; \
         DELETE FROM device_one_time_keys WHERE account_id = '{b}' AND device_id = {bd}; \
         INSERT INTO device_signed_keys SELECT * FROM {backup}_s; INSERT INTO device_one_time_keys SELECT * FROM {backup}_o; \
         DROP TABLE {backup}_s; DROP TABLE {backup}_o;"
    )
}

/// Issue I8 of docs/SECURITY-REVIEW-CLIENT.md: on first contact the client used to trust
/// whatever key the server's directory served. With key transparency the key must be the one
/// in the public log, and a fake key without a log entry is refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn first_contact_with_a_key_not_in_the_log_is_refused() {
    if !enabled() {
        return;
    }
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (alice, bob, mallory) = (Client::new("alice"), Client::new("bob"), Client::new("mallory"));
    let (_a, b, m) = (alice.register().await, bob.register().await, mallory.register().await);

    // Before Alice ever talks to Bob, the server swaps Bob's bundle for Mallory's.
    let undo = swap_bundle(&b, &m);
    let (bid, conv) = alice.conversation_with(&b.number).await;
    let refused = send_text(&alice, &conv, "secret for bob only").await;
    alice.wait_status(&refused.id, "failed").await;
    alice.wait_kt_alert("key_mismatch").await;
    let id = bid.clone();
    alice.wait_event("identity conflict", move |n, v| n == "identity_conflict" && v["account_id"].as_str() == Some(id.as_str())).await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(!bob.saw_message("secret for bob only"));
    let queued = sql(&format!("SELECT COUNT(*) FROM envelopes WHERE account_id = '{}'", b.account_id));
    assert_eq!(queued, "0", "nothing may be encrypted for the fake key");

    // The server is fixed: the real key is in the log, messages flow and the key is verified.
    sql(&undo);
    send_text(&alice, &conv, "all good again").await;
    bob.wait_message("all good again").await;
    assert_eq!(alice.kt_contact(&bid).await, "verified");
}

/// A server that also forges a log entry for the fake key fools the first contact, but not the
/// account's owner: its self-monitoring finds an entry with its account and a key that is not
/// its own.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_forged_log_entry_is_found_by_the_owner() {
    if !enabled() {
        return;
    }
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (alice, bob, mallory) = (Client::new("alice"), Client::new("bob"), Client::new("mallory"));
    let (_a, b, m) = (alice.register().await, bob.register().await, mallory.register().await);
    let overview = bob.kt_check().await;
    assert_eq!(overview.own_status, "verified", "{overview:?}");

    let undo = swap_bundle(&b, &m);
    // The forged entry: Bob's account with Mallory's identity key, appended to the log.
    sql(&format!(
        "INSERT INTO kt_leaves (idx, account_id, identity_key, created_at) \
           SELECT s.size, '{}', a.identity_key, a.created_at FROM kt_state s, accounts a WHERE s.id = 1 AND a.id = '{}'; \
         UPDATE kt_state SET size = size + 1 WHERE id = 1;",
        b.account_id, m.account_id
    ));
    let (bid, conv) = alice.conversation_with(&b.number).await;
    let fooled = send_text(&alice, &conv, "fooled").await;
    alice.wait_status(&fooled.id, "sent").await;
    assert_eq!(alice.kt_contact(&bid).await, "verified", "the forged entry has a valid proof");

    // Bob's self-check sees the foreign entry for his account.
    let _ = bob.kt_check().await;
    bob.wait_kt_alert("own_key_mismatch").await;
    let overview = bob.kt_check().await;
    assert_eq!(overview.own_status, "alert", "{overview:?}");
    sql(&undo);
}

/// The server rewrites an entry of its log after Alice saw it, and shows the rewritten log to
/// Bob (each view is consistent on its own). Bob's next message carries his tree head: Alice
/// sees two signed heads of the same size with different roots, and raises a split-view alert.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_split_view_is_exposed_by_gossip() {
    if !enabled() {
        return;
    }
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (alice, bob, carol) = (Client::new("alice"), Client::new("bob"), Client::new("carol"));
    let (a, b) = (alice.register().await, bob.register().await);
    let (bid, conv) = alice.conversation_with(&b.number).await;
    send_text(&alice, &conv, "hi bob").await;
    let bob_conv = bob.wait_message("hi bob").await["conversation_id"].as_str().unwrap().to_string();
    assert_eq!(bob.kt_contact(&a.account_id).await, "verified");
    assert_eq!(alice.kt_contact(&bid).await, "verified");

    // A new account joins the log; Alice sees it.
    let c = carol.register().await;
    let seen_by_alice = alice.kt_check().await;
    let last = sql("SELECT account_id FROM kt_leaves ORDER BY idx DESC LIMIT 1");
    assert_eq!(last, c.account_id, "carol's entry must be the last one");
    assert_eq!(seen_by_alice.tree_size.map(|s| s.to_string()), Some(sql("SELECT size FROM kt_state WHERE id = 1")));

    // The server rewrites Carol's entry (the last one) and shows the new log to Bob.
    let original = sql(&format!("SELECT HEX(identity_key) FROM kt_leaves WHERE account_id = '{}'", c.account_id));
    sql(&format!(
        "UPDATE kt_leaves SET identity_key = UNHEX(CONCAT('05', REPEAT('AB', 32))) WHERE account_id = '{}'",
        c.account_id
    ));
    let seen_by_bob = bob.kt_check().await;
    assert_eq!(seen_by_bob.tree_size, seen_by_alice.tree_size);
    assert!(bob.kt_alerts().is_empty(), "Bob's view is consistent on its own: {:?}", bob.kt_alerts());

    // Bob writes to Alice: his head travels with the message.
    send_text(&bob, &bob_conv, "hello alice").await;
    alice.wait_message("hello alice").await;
    let alerts = alice.wait_kt_alert("split_view").await;
    let alert = alerts.as_array().unwrap().iter().find(|x| x["kind"] == "split_view").unwrap().clone();
    assert_eq!(alert["account_id"].as_str(), Some(b.account_id.as_str()), "the alert names who gossiped the other view");
    let (ours, theirs) = (&alert["evidence"]["ours"], &alert["evidence"]["theirs"]);
    assert_eq!(ours["tree_size"], theirs["tree_size"]);
    assert_ne!(ours["root_hash"], theirs["root_hash"], "two signed roots for one size: proof of the split view");

    sql(&format!("UPDATE kt_leaves SET identity_key = UNHEX('{original}') WHERE account_id = '{}'", c.account_id));
}
