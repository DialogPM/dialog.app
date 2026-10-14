//! Disappearing messages apply to the whole conversation: turning the timer on also expires
//! the messages already there (each counted from when it was sent), on both sides.
//!
//! `CHATAPP_E2E_SERVER=http://127.0.0.1:8081 cargo test -p chat-core --test e2e_disappearing`

use std::sync::{Arc, Mutex};
use std::time::Duration;

use chat_core::model::*;
use chat_core::{Core, CoreConfig, EventSink};
use serde_json::{json, Value};

#[derive(Default)]
struct Events(Mutex<Vec<(String, Value)>>);

impl EventSink for Events {
    fn emit(&self, event: &str, payload: Value) {
        self.0.lock().unwrap().push((event.to_string(), payload));
    }
}

struct Client {
    core: Core,
    events: Arc<Events>,
    _dir: tempfile::TempDir,
}

impl Client {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let events = Arc::new(Events::default());
        let core = Core::start(CoreConfig {
            data_dir: dir.path().to_path_buf(),
            db_key: [9u8; 32],
            default_server_url: server(),
            platform: "cli".into(),
            events: events.clone(),
        })
        .unwrap();
        Self { core, events, _dir: dir }
    }

    async fn wait(&self, what: &str, pred: impl Fn(&str, &Value) -> bool) -> Value {
        for _ in 0..300 {
            if let Some((_, v)) = self.events.0.lock().unwrap().iter().find(|(n, v)| pred(n, v)) {
                return v.clone();
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("timed out waiting for {what}");
    }

    async fn message(&self, id: &str) -> Option<Message> {
        let id = id.to_string();
        self.core.run(move |i| async move { i.get_message(&id) }).await.ok()
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
async fn timer_also_expires_existing_messages() {
    if server().is_empty() {
        eprintln!("CHATAPP_E2E_SERVER not set, skipping");
        return;
    }
    let (alice, bob) = (Client::new(), Client::new());
    let mut accounts = Vec::new();
    for c in [&alice, &bob] {
        let u = server();
        let r = c.core.run(move |i| async move { i.register(&u, "disappearing test pw", "test").await }).await.unwrap();
        c.wait("online", |n, v| n == "connection" && v.as_str() == Some("online")).await;
        accounts.push(r.account);
    }
    let (bnum, bid) = (accounts[1].number.clone(), accounts[1].account_id.clone());
    alice.core.run(move |i| async move { i.add_contact_by_number(&bnum, Some("Bob".into())).await }).await.unwrap();
    let conv = alice.core.run(move |i| async move { i.open_direct_conversation(&bid).await }).await.unwrap().id;

    // Two messages before any timer; Bob accepts the chat and answers.
    let old = send(&alice, &conv, "old message").await;
    let got = bob.wait("old message", |n, v| n == "message" && v["message"]["text"] == "old message").await;
    let bob_conv = got["message"]["conversation_id"].as_str().unwrap().to_string();
    let bc = bob_conv.clone();
    bob.core.run(move |i| async move { i.accept_conversation(&bc).await }).await.unwrap();
    let reply = send(&bob, &bob_conv, "old reply").await;
    alice.wait("old reply", |n, v| n == "message" && v["message"]["text"] == "old reply").await;
    assert!(old.expires_at.is_none() && reply.expires_at.is_none());

    // Timer of 2 s: everything already older than that goes at once, on both sides.
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let patch: ConversationSettingsPatch =
        serde_json::from_value(json!({ "conversation_id": conv, "disappearing_seconds": 2 })).unwrap();
    alice.core.run(move |i| async move { i.set_conversation_settings(patch).await }).await.unwrap();
    let (oid, rid) = (old.id.clone(), reply.id.clone());
    alice.wait("old message gone (alice)", move |n, v| n == "message_deleted" && v["message_id"] == oid.as_str()).await;
    let rid2 = rid.clone();
    alice.wait("reply gone (alice)", move |n, v| n == "message_deleted" && v["message_id"] == rid2.as_str()).await;
    let oid = old.id.clone();
    bob.wait("old message gone (bob)", move |n, v| n == "message_deleted" && v["message_id"] == oid.as_str()).await;
    bob.wait("reply gone (bob)", move |n, v| n == "message_deleted" && v["message_id"] == rid.as_str()).await;
    assert!(alice.message(&old.id).await.is_none(), "the row is deleted, not hidden");
    assert!(bob.message(&reply.id).await.is_none(), "the row is deleted, not hidden");

    // New messages follow the timer as before.
    let fresh = send(&alice, &conv, "fresh").await;
    assert!(fresh.expires_at.is_some());
}
