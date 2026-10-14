//! Default disappearing timer (engine/default_timer.rs): conversations and groups we start get
//! it from the start and the peer is told; existing conversations and conversations started by
//! others keep their own timer.
//!
//! `CHATAPP_E2E_SERVER=http://127.0.0.1:8081 cargo test -p chat-core --test e2e_default_timer`

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

    async fn conversation(&self, id: &str) -> Conversation {
        let id = id.to_string();
        self.core.run(move |i| async move { i.get_conversation(&id) }).await.unwrap()
    }

    /// Adds `peer` as a contact and opens (starts) the conversation with them.
    async fn open(&self, peer: &AccountInfo) -> Conversation {
        let (num, id) = (peer.number.clone(), peer.account_id.clone());
        self.core.run(move |i| async move { i.add_contact_by_number(&num, None).await }).await.unwrap();
        self.core.run(move |i| async move { i.open_direct_conversation(&id).await }).await.unwrap()
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
async fn new_chats_and_groups_start_with_the_default_timer() {
    if server().is_empty() {
        eprintln!("CHATAPP_E2E_SERVER not set, skipping");
        return;
    }
    let (alice, bob, carol, dave) = (Client::new(), Client::new(), Client::new(), Client::new());
    let mut acc = Vec::new();
    for c in [&alice, &bob, &carol, &dave] {
        let u = server();
        let r = c.core.run(move |i| async move { i.register(&u, "default timer test pw", "test").await }).await.unwrap();
        c.wait("online", |n, v| n == "connection" && v.as_str() == Some("online")).await;
        acc.push(r.account);
    }

    // A conversation that exists before the default is set keeps its timer (off).
    let with_carol = alice.open(&acc[2]).await;
    assert_eq!(with_carol.disappearing_seconds, 0);
    send(&alice, &with_carol.id, "before the default").await;

    let st = alice.core.run(|i| async move { i.set_default_disappearing(3600).await }).await.unwrap();
    assert_eq!(st.seconds, 3600);
    let err = alice.core.run(|i| async move { i.set_default_disappearing(7200).await }).await.unwrap_err();
    assert_eq!(err.code, chat_core::error::ErrorCode::InvalidInput);
    assert_eq!(alice.core.run(|i| async move { Ok(i.get_default_disappearing()) }).await.unwrap().seconds, 3600);

    // A new conversation Alice starts gets the timer, and Bob is told.
    let with_bob = alice.open(&acc[1]).await;
    assert_eq!(with_bob.disappearing_seconds, 3600, "a chat we start gets the default");
    let msg = send(&alice, &with_bob.id, "hello bob").await;
    assert!(msg.expires_at.is_some(), "the first message already disappears");
    let notice = bob
        .wait("timer notice", |n, v| n == "message" && v["message"]["system"]["type"] == "disappearing_changed")
        .await;
    let bob_conv = notice["message"]["conversation_id"].as_str().unwrap().to_string();
    assert_eq!(notice["message"]["system"]["value"], "3600");
    let got = bob.wait("hello bob", |n, v| n == "message" && v["message"]["text"] == "hello bob").await;
    assert_eq!(got["message"]["conversation_id"].as_str(), Some(bob_conv.as_str()));
    assert_eq!(bob.conversation(&bob_conv).await.disappearing_seconds, 3600);
    // Received messages start their timer once read.
    let (bc, mid) = (bob_conv.clone(), got["message"]["id"].as_str().unwrap().to_string());
    let read = bob.core.run(move |i| async move { i.mark_read(&bc)?; i.get_message(&mid) }).await.unwrap();
    assert!(read.expires_at.is_some(), "Bob's copy disappears too");

    // Reopening the existing conversation changes nothing.
    let again = alice.open(&acc[2]).await;
    assert_eq!((again.id.as_str(), again.disappearing_seconds), (with_carol.id.as_str(), 0));
    assert!(send(&alice, &with_carol.id, "still no timer").await.expires_at.is_none());
    let reopened = alice.open(&acc[1]).await;
    assert_eq!(reopened.disappearing_seconds, 3600);

    // A group Alice creates starts with the default as its group timer.
    let members = vec![acc[1].account_id.clone(), acc[2].account_id.clone()];
    let req: CreateGroupRequest = serde_json::from_value(json!({ "name": "timed", "member_account_ids": members })).unwrap();
    let g = alice.core.run(move |i| async move { i.create_group(req).await }).await.unwrap();
    assert_eq!(g.disappearing_seconds, 3600);
    bob.wait("group with timer", |n, v| n == "conversation_updated" && v["title"] == "timed" && v["disappearing_seconds"] == 3600)
        .await;
    // An explicit choice in the request wins over the default.
    let members = vec![acc[1].account_id.clone()];
    let req: CreateGroupRequest =
        serde_json::from_value(json!({ "name": "untimed", "member_account_ids": members, "settings": { "disappearing_seconds": 0 } })).unwrap();
    let g = alice.core.run(move |i| async move { i.create_group(req).await }).await.unwrap();
    assert_eq!(g.disappearing_seconds, 0);

    // A conversation started by someone else (Dave, default off) has no timer on either side.
    let dave_conv = dave.open(&acc[0]).await;
    assert_eq!(dave_conv.disappearing_seconds, 0);
    assert!(send(&dave, &dave_conv.id, "hi from dave").await.expires_at.is_none());
    let got = alice.wait("dave's message", |n, v| n == "message" && v["message"]["text"] == "hi from dave").await;
    assert!(got["message"]["expires_at"].is_null());
    let alice_dave = got["message"]["conversation_id"].as_str().unwrap().to_string();
    assert_eq!(alice.conversation(&alice_dave).await.disappearing_seconds, 0, "chats started by others are not affected");
    // Alice opening it afterwards does not change it either.
    assert_eq!(alice.open(&acc[3]).await.disappearing_seconds, 0);
}
