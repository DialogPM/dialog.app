//! A first message that gets no answer for more than 30 days: libsignal then stops using that
//! session ("session not found" when encrypting). The next message must start a new session
//! instead of failing for good.
//!
//! `CHATAPP_E2E_SERVER=http://127.0.0.1:8081 cargo test -p chat-core --features insecure-test-hooks --test e2e_stale_session`

use std::sync::{Arc, Mutex};
use std::time::Duration;

use chat_core::model::*;
use chat_core::{Core, CoreConfig, EventSink};
use serde_json::{json, Value};

const PASSWORD: &str = "correct horse battery";

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
    dir: Arc<tempfile::TempDir>,
}

impl Client {
    fn at(name: &'static str, dir: Arc<tempfile::TempDir>) -> Self {
        let events = Arc::new(Events::default());
        let core = Core::start(CoreConfig {
            data_dir: dir.path().to_path_buf(),
            db_key: [7u8; 32],
            default_server_url: server(),
            platform: "cli".into(),
            events: events.clone(),
        })
        .unwrap();
        Self { name, core, events, dir }
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

    async fn register(&self) -> AccountInfo {
        let u = server();
        let r = self.core.run(move |i| async move { i.register(&u, PASSWORD, "test device").await }).await.unwrap();
        self.wait_online().await;
        r.account
    }
}

fn server() -> String {
    std::env::var("CHATAPP_E2E_SERVER").unwrap_or_default()
}

async fn send(c: &Client, conv: &str, text: &str) -> Message {
    let req: SendMessageRequest = serde_json::from_value(json!({ "conversation_id": conv, "text": text })).unwrap();
    c.core.run(move |i| async move { i.send_message(req).await }).await.unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_session_never_answered_for_a_month_is_started_again() {
    if server().is_empty() {
        eprintln!("CHATAPP_E2E_SERVER not set, skipping");
        return;
    }
    let alice = Client::at("alice", Arc::new(tempfile::tempdir().unwrap()));
    let bob = Client::at("bob", Arc::new(tempfile::tempdir().unwrap()));
    let _a = alice.register().await;
    let b = bob.register().await;

    // Bob's app is closed: he neither answers nor acknowledges anything.
    bob.core.run(|i| async move { i.stop_background(); Ok(()) }).await.unwrap();
    let bob_dir = bob.dir.clone();
    drop(bob);
    tokio::time::sleep(Duration::from_millis(800)).await;

    let (num, id) = (b.number.clone(), b.account_id.clone());
    alice.core.run(move |i| async move { i.add_contact_by_number(&num, None).await }).await.unwrap();
    let conv = alice.core.run(move |i| async move { i.open_direct_conversation(&id).await }).await.unwrap().id;
    let first = send(&alice, &conv, "first, never answered").await;
    let fid = first.id.clone();
    alice.wait("first message sent", move |n, v| n == "message" && v["message"]["id"] == fid.as_str() && v["message"]["status"] == "sent").await;

    // 31 days later, still no answer: Alice writes again.
    alice.core.run(|i| async move { i.insecure_advance_session_clock(Duration::from_secs(31 * 24 * 3600)); Ok(()) }).await.unwrap();
    let second = send(&alice, &conv, "second, a month later").await;
    let sid = second.id.clone();
    alice
        .wait("second message sent", move |n, v| n == "message" && v["message"]["id"] == sid.as_str() && v["message"]["status"] == "sent")
        .await;

    // Bob opens his app at last and reads both.
    let bob = Client::at("bob", bob_dir);
    if bob.core.run(|i| async move { Ok(i.app_state().status) }).await.unwrap() == "locked" {
        bob.core.run(|i| async move { i.unlock(PASSWORD).await }).await.unwrap();
    }
    bob.wait_online().await;
    for text in ["first, never answered", "second, a month later"] {
        bob.wait(text, move |n, v| n == "message" && v["message"]["text"] == text).await;
    }
}
