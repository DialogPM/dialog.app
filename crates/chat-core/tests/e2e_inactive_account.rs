//! Automatic deletion of inactive accounts against a real server: an account none of whose
//! devices connected for the period its user chose is deleted by the server's maintenance
//! job; "never", a period not yet over and an open connection keep it; the recovery phrase
//! of an account deleted that way gives back the same number, account id and identity, and
//! its contacts can write to it again. An account deleted by its user stays deleted.
//!
//! The inactivity is simulated by moving the account's dates back in the server's database,
//! and the server must run its jobs often (`CHATAPP_JOB_INTERVAL_SECONDS`, set by
//! `scripts/test-e2e.sh`).
//!
//! `scripts/test-e2e.sh --test e2e_inactive_account`

use std::sync::{Arc, Mutex};
use std::time::Duration;

use chat_core::model::*;
use chat_core::{Core, CoreConfig, ErrorCode, EventSink};
use serde_json::{json, Value};

const PASSWORD: &str = "correct horse battery";
const DAY_MS: i64 = 86_400_000;

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

    async fn wait(&self, what: &str, pred: impl Fn(&str, &Value) -> bool) -> Value {
        for _ in 0..300 {
            if let Some((_, v)) = self.events.0.lock().unwrap().iter().find(|(n, v)| pred(n, v)) {
                return v.clone();
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("{}: timed out waiting for {what}", self.name);
    }

    async fn wait_online(&self) {
        self.wait("online", |n, v| n == "connection" && v.as_str() == Some("online")).await;
    }

    async fn wait_text(&self, text: &'static str) -> Value {
        self.wait(text, move |n, v| n == "message" && v["message"]["text"] == text).await
    }

    /// Registers and returns the account with its 24 words.
    async fn register(&self) -> (AccountInfo, Vec<String>) {
        let u = server();
        let r = self.core.run(move |i| async move { i.register(&u, PASSWORD, "test device").await }).await.unwrap();
        self.wait_online().await;
        (r.account, r.mnemonic)
    }

    async fn period(&self) -> Option<i64> {
        self.core.run(|i| async move { i.account_details().await }).await.unwrap().inactive_delete_days
    }

    async fn set_period(&self, days: i64) -> chat_core::Result<i64> {
        self.core.run(move |i| async move { i.set_inactive_deletion(days).await }).await
    }
}

fn server() -> String {
    std::env::var("CHATAPP_E2E_SERVER").unwrap_or_default()
}

/// Runs SQL on the server's database.
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

fn exists(account: &AccountInfo) -> bool {
    sql(&format!("SELECT COUNT(*) FROM accounts WHERE id = '{}'", account.account_id)) == "1"
}

/// As if nothing of the account had been seen for `days` days.
fn idle_for(account: &AccountInfo, days: i64) {
    sql(&format!(
        "UPDATE accounts SET last_active_at = last_active_at - {ms}, created_at = created_at - {ms} WHERE id = '{id}'",
        ms = days * DAY_MS,
        id = account.account_id
    ));
}

async fn send(c: &Client, conv: &str, text: &str) -> chat_core::Result<Message> {
    let req: SendMessageRequest = serde_json::from_value(json!({ "conversation_id": conv, "text": text })).unwrap();
    c.core.run(move |i| async move { i.send_message(req).await }).await
}

async fn recover(dev: &Client, number: &str, words: &[String], password: &'static str) -> chat_core::Result<AccountInfo> {
    let (u, number, words) = (server(), number.to_string(), words.to_vec());
    dev.core.run(move |i| async move { i.recover(&u, &number, &words, password, "restored device").await }).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn inactive_accounts_are_deleted_and_restored_from_the_recovery_phrase() {
    if server().is_empty() || std::env::var("CHATAPP_E2E_MYSQL").is_err() {
        eprintln!("CHATAPP_E2E_SERVER / CHATAPP_E2E_MYSQL not set, skipping");
        return;
    }
    let _ = env_logger::builder().is_test(true).try_init();

    // ana: the default (a year), away for longer. nora: "never". carol: 30 days, away 31.
    // dave: a year, away 31 days. erin: 30 days, "away" 31 but with her app open. bob: active.
    let (ana, nora, carol, dave, erin, bob) =
        (Client::new("ana"), Client::new("nora"), Client::new("carol"), Client::new("dave"), Client::new("erin"), Client::new("bob"));
    let (a, a_words) = ana.register().await;
    let (n, _) = nora.register().await;
    let (c, c_words) = carol.register().await;
    let (d, d_words) = dave.register().await;
    let (e, _) = erin.register().await;
    let (b, _) = bob.register().await;

    // --- the setting ---------------------------------------------------------------------
    assert_eq!(ana.period().await, Some(365), "a new account is deleted after a year of inactivity");
    assert_eq!(nora.set_period(0).await.unwrap(), 0);
    assert_eq!(nora.period().await, Some(0));
    assert_eq!(carol.set_period(30).await.unwrap(), 30);
    assert_eq!(erin.set_period(30).await.unwrap(), 30);
    for bad in [-1, 1, 45, 366, 100_000] {
        assert!(dave.set_period(bad).await.is_err(), "{bad} days is not offered");
    }
    assert_eq!(dave.period().await, Some(365));
    for days in [60, 90, 730, 1095, 1825, 365] {
        assert_eq!(dave.set_period(days).await.unwrap(), days);
    }

    // Ana has a username, and she and Bob talk.
    ana.core.run(|i| async move { i.set_username(Some(format!("ana_{}", &uuid::Uuid::new_v4().simple().to_string()[..10]))).await }).await.unwrap();
    let (num, id) = (a.number.clone(), a.account_id.clone());
    bob.core.run(move |i| async move { i.add_contact_by_number(&num, None).await }).await.unwrap();
    let bob_conv = bob.core.run(move |i| async move { i.open_direct_conversation(&id).await }).await.unwrap().id;
    send(&bob, &bob_conv, "hello ana").await.unwrap();
    let got = ana.wait_text("hello ana").await;
    let ana_conv = got["conversation_id"].as_str().unwrap().to_string();
    send(&ana, &ana_conv, "hello bob").await.unwrap();
    bob.wait_text("hello bob").await;

    // --- time passes ---------------------------------------------------------------------
    // Everyone but Erin and Bob closes the app (the socket's end is recorded as activity, so
    // the dates are moved only once the server has seen it).
    for cl in [ana, nora, carol, dave] {
        cl.core.shutdown().await;
    }
    tokio::time::sleep(Duration::from_secs(3)).await;
    idle_for(&a, 400);
    idle_for(&n, 4000);
    idle_for(&c, 31);
    idle_for(&d, 31);
    idle_for(&e, 31);

    // The server's job deletes Ana and Carol, and only them.
    for _ in 0..600 {
        if !exists(&a) && !exists(&c) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(!exists(&a), "an account away for more than its year must be deleted");
    assert!(!exists(&c), "an account away for more than its 30 days must be deleted");
    // A few more runs of the job.
    tokio::time::sleep(Duration::from_secs(6)).await;
    assert!(exists(&n), "\"never\" keeps the account");
    assert!(exists(&d), "31 days are not a year");
    assert!(exists(&e), "an account with an open connection is active");
    assert!(exists(&b));
    let erin_active: i64 = sql(&format!("SELECT last_active_at FROM accounts WHERE id = '{}'", e.account_id)).parse().unwrap();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64;
    assert!(now - erin_active < DAY_MS, "the open connection counts as activity now");

    // Deleted for real: devices, keys, profile and username are gone; the number stays taken.
    for table in ["devices", "profiles", "device_signed_keys"] {
        assert_eq!(sql(&format!("SELECT COUNT(*) FROM {table} WHERE account_id = '{}'", a.account_id)), "0", "{table}");
    }
    assert_eq!(sql(&format!("SELECT COUNT(*) FROM account_numbers WHERE number = '{}' AND deleted_at IS NOT NULL", a.number)), "1");
    // The old password no longer opens anything.
    let stranger = Client::new("stranger");
    let (u, number) = (server(), a.number.clone());
    let err = stranger.core.run(move |i| async move { i.login(&u, &number, PASSWORD, "x", None).await }).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidPassword);
    // Bob cannot reach her.
    let _ = send(&bob, &bob_conv, "are you there?").await;

    // --- the recovery phrase brings the account back ---------------------------------------
    // Not someone else's words.
    let err = recover(&stranger, &a.number, &d_words, "Another-password-1").await.unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidMnemonic);
    assert!(!exists(&a));

    const NEW_PASSWORD: &str = "Restored-password-2";
    let ana2 = Client::new("ana2");
    let restored = recover(&ana2, &a.number, &a_words, NEW_PASSWORD).await.unwrap();
    assert_eq!(restored.number, a.number, "the same account number");
    assert_eq!(restored.account_id, a.account_id, "the same account");
    ana2.wait_online().await;
    assert_eq!(ana2.core.run(|i| async move { i.reveal_mnemonic(NEW_PASSWORD).await }).await.unwrap(), a_words);
    let details = ana2.core.run(|i| async move { i.account_details().await }).await.unwrap();
    assert_eq!(details.inactive_delete_days, Some(365));
    assert_eq!(details.username, None, "the username was deleted with the account");
    assert_eq!(
        sql(&format!("SELECT COUNT(*) FROM account_numbers WHERE number = '{}' AND deleted_at IS NULL AND recovery_public_key IS NULL", a.number)),
        "1",
        "nothing of the deleted account is kept once it is restored"
    );
    // The same identity: one entry in the key transparency log, as before.
    assert_eq!(sql(&format!("SELECT COUNT(DISTINCT identity_key) FROM kt_leaves WHERE account_id = '{}'", a.account_id)), "1");

    // Bob still has her as a contact and they talk again, in the conversation he had.
    send(&bob, &bob_conv, "welcome back").await.unwrap();
    let got = ana2.wait_text("welcome back").await;
    let conv = got["conversation_id"].as_str().unwrap().to_string();
    send(&ana2, &conv, "good to be back").await.unwrap();
    let got = bob.wait_text("good to be back").await;
    assert_eq!(got["conversation_id"].as_str(), Some(bob_conv.as_str()));

    // Carol's words work as well, and a restored account is an ordinary one: it logs in.
    let carol2 = Client::new("carol2");
    let restored = recover(&carol2, &c.number, &c_words, NEW_PASSWORD).await.unwrap();
    assert_eq!(restored.account_id, c.account_id);
    assert_eq!(carol2.period().await, Some(365), "the period starts again from the default");
    let carol3 = Client::new("carol3");
    let (u, number) = (server(), c.number.clone());
    let acc = carol3.core.run(move |i| async move { i.login(&u, &number, NEW_PASSWORD, "second", None).await }).await.unwrap();
    assert_eq!(acc.account_id, c.account_id);

    // --- an account deleted by its user stays deleted ---------------------------------------
    let zed = Client::new("zed");
    let (z, z_words) = zed.register().await;
    zed.core.run(|i| async move { i.delete_account(PASSWORD).await }).await.unwrap();
    assert!(!exists(&z));
    let zed2 = Client::new("zed2");
    let err = recover(&zed2, &z.number, &z_words, NEW_PASSWORD).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidMnemonic);
    assert!(!exists(&z));
}
