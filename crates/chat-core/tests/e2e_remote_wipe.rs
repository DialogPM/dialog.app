//! Remote wipe (crate::revocation) against a real server: a phone keeps two accounts, locked
//! with their passwords (as after a restart). One of them removes the phone from another
//! device; the phone, still locked, finds out and deletes that account's data without its
//! password, and keeps the other account. Then the same through an account recovery.
//!
//! `CHATAPP_E2E_SERVER=http://127.0.0.1:8081 cargo test -p chat-core --test e2e_remote_wipe`

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chat_core::model::*;
use chat_core::profiles::Profiles;
use chat_core::{Core, CoreConfig, EventSink};
use serde_json::{json, Value};

#[derive(Default)]
struct Events(Mutex<Vec<(String, Value)>>);

impl EventSink for Events {
    fn emit(&self, event: &str, payload: Value) {
        self.0.lock().unwrap().push((event.to_string(), payload));
    }
}

impl Events {
    async fn wait(&self, what: &str, pred: impl Fn(&str, &Value) -> bool) {
        for _ in 0..300 {
            if self.0.lock().unwrap().iter().any(|(n, v)| pred(n, v)) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("timed out waiting for {what}");
    }

    fn clear(&self) {
        self.0.lock().unwrap().clear();
    }
}

fn server() -> String {
    std::env::var("CHATAPP_E2E_SERVER").unwrap_or_default()
}

const PW_A: &str = "Remote-wipe-account-A-1";
const PW_B: &str = "Remote-wipe-account-B-2";

/// Creates an account on the phone's active profile and locks its data with the password.
async fn create_account(phone: &Profiles, events: &Events, password: &'static str) -> RegisterResult {
    let url = server();
    let r = phone.core().run(move |i| async move { i.register(&url, password, "Phone").await }).await.unwrap();
    phone.invoke("confirm_mnemonic_saved", Value::Null).await.unwrap();
    phone.invoke("protect_local_data", json!({ "password": password })).await.unwrap();
    events.wait("online", |n, v| n == "connection" && v.as_str() == Some("online")).await;
    r
}

/// Waits until the phone registered its remote-wipe secret for the active profile.
async fn wait_revocation_registered(dir: &Path) {
    for _ in 0..100 {
        if let Ok(raw) = std::fs::read(dir.join(chat_core::revocation::FILE)) {
            if serde_json::from_slice::<Value>(&raw).ok().and_then(|v| v["registered"].as_bool()) == Some(true) {
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("remote wipe secret never registered");
}

/// Another device (a separate core) of an account.
fn other_device(dir: &Path) -> Core {
    Core::start(CoreConfig {
        data_dir: dir.to_path_buf(),
        db_key: [7u8; 32],
        default_server_url: server(),
        platform: "cli".into(),
        events: Arc::new(Events::default()),
    })
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn removed_device_wipes_itself_while_locked() {
    if server().is_empty() {
        eprintln!("CHATAPP_E2E_SERVER not set, skipping");
        return;
    }
    let _ = env_logger::builder().try_init();
    let base = tempfile::tempdir().unwrap();
    let events = Arc::new(Events::default());
    let phone = Profiles::start(base.path().to_path_buf(), server(), "ios".into(), events.clone()).unwrap();

    // Two accounts on the phone, each registering its remote-wipe secret.
    let a = create_account(&phone, &events, PW_A).await;
    wait_revocation_registered(&phone.data_dir()).await;
    let a_dir = phone.data_dir();
    phone.invoke("profile_close", Value::Null).await.unwrap();
    events.clear();
    let b = create_account(&phone, &events, PW_B).await;
    wait_revocation_registered(&phone.data_dir()).await;
    let b_dir = phone.data_dir();
    let profile_of = |p: &Profiles, n: &str| p.list().into_iter().find(|c| c.last4.as_deref() == Some(&n[12..])).map(|c| c.id);
    let a_profile = profile_of(&phone, &a.account.number).expect("A saved on the phone");
    let b_profile = profile_of(&phone, &b.account.number).expect("B saved on the phone");

    // The phone restarts: both accounts are locked (their data needs the password).
    phone.exit().await;
    let phone = Profiles::start(base.path().to_path_buf(), server(), "ios".into(), events.clone()).unwrap();
    assert_eq!(phone.list().len(), 2);
    assert_eq!(phone.check_revoked().await, Vec::<String>::new(), "nothing removed yet");

    // A logs in on a computer and removes the phone from the account.
    let pc_dir = tempfile::tempdir().unwrap();
    let pc = other_device(pc_dir.path());
    let (url, number) = (server(), a.account.number.clone());
    pc.run(move |i| async move { i.login(&url, &number, PW_A, "Computer", None).await }).await.unwrap();
    let phone_device = a.account.device_id;
    pc.run(move |i| async move { i.unlink_device(phone_device).await }).await.unwrap();

    // The locked phone finds out and deletes A's data, without its password; B stays.
    let wiped = phone.check_revoked().await;
    assert_eq!(wiped, vec![a_profile.clone()]);
    assert!(!a_dir.join("chat.db").exists() && !a_dir.exists(), "A's data is gone");
    assert!(b_dir.join("chat.db").exists(), "B's data is kept");
    let left: Vec<String> = phone.list().into_iter().map(|c| c.id).collect();
    assert_eq!(left, vec![b_profile.clone()]);
    assert_eq!(phone.check_revoked().await, Vec::<String>::new(), "only once");
    pc.shutdown().await;

    // B is recovered on another device with its recovery words: the phone (B's old device)
    // wipes B too, here while B is open on the phone.
    phone.invoke("profile_open", json!({ "id": b_profile, "password": PW_B })).await.unwrap();
    events.clear();
    let pc2_dir = tempfile::tempdir().unwrap();
    let pc2 = other_device(pc2_dir.path());
    let (url, number, words) = (server(), b.account.number.clone(), b.mnemonic.clone());
    pc2.run(move |i| async move { i.recover(&url, &number, &words, "Recovered-B-password-3", "Laptop").await }).await.unwrap();
    // The open core may notice first (its connection is refused) and wipe itself: either way
    // the data goes, once.
    let wiped = phone.check_revoked().await;
    assert!(wiped.is_empty() || wiped == vec![b_profile], "{wiped:?}");
    events
        .wait("logged out", |n, v| n == "logged_out" && v["reason"].as_str() == Some("device_removed"))
        .await;
    assert!(!b_dir.join("chat.db").exists(), "B's data is gone");
    assert!(phone.list().is_empty());
    pc2.shutdown().await;
    phone.exit().await;
}
