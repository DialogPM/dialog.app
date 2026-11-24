//! Restoring an account on a new device: the contact as the other devices know it (name,
//! blocked) must arrive whatever the order of the vault records. A message of someone is often
//! stored before their contact record (they wrote first, we named or blocked them later), and
//! applying the message creates a bare contact row first.
//!
//! `CHATAPP_E2E_SERVER=http://127.0.0.1:8081 cargo test -p chat-core --test e2e_restore_contacts`

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
    _dir: Arc<tempfile::TempDir>,
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
async fn contacts_named_or_blocked_after_their_first_message_are_restored() {
    if server().is_empty() {
        eprintln!("CHATAPP_E2E_SERVER not set, skipping");
        return;
    }
    let dir = || Arc::new(tempfile::tempdir().unwrap());
    let (alice, bob, carol) = (Client::at("alice", dir()), Client::at("bob", dir()), Client::at("carol", dir()));
    let a = alice.register().await;
    let b = bob.register().await;
    let c = carol.register().await;

    // Bob and Carol write first: Alice has their messages before she has them as contacts.
    for (who, text) in [(&bob, "hello from bob"), (&carol, "hello from carol")] {
        let (num, id) = (a.number.clone(), a.account_id.clone());
        who.core.run(move |i| async move { i.add_contact_by_number(&num, None).await }).await.unwrap();
        let conv = who.core.run(move |i| async move { i.open_direct_conversation(&id).await }).await.unwrap().id;
        send(who, &conv, text).await;
        let got = alice.wait(text, move |n, v| n == "message" && v["message"]["text"] == text).await;
        let conv = got["message"]["conversation_id"].as_str().unwrap().to_string();
        alice.core.run(move |i| async move { i.accept_conversation(&conv).await }).await.unwrap();
        let more = format!("{text}, again");
        send(who, &conv_of(who, &a.account_id).await, &more).await;
        alice.wait("the second message", move |n, v| n == "message" && v["message"]["text"] == more.as_str()).await;
    }
    // The messages reach the vault first.
    tokio::time::sleep(Duration::from_secs(4)).await;
    // Later Alice names Bob and blocks Carol.
    let num = b.number.clone();
    alice.core.run(move |i| async move { i.add_contact_by_number(&num, Some("Bobby".into())).await }).await.unwrap();
    let id = c.account_id.clone();
    alice.core.run(move |i| async move { i.set_blocked(&id, true) }).await.unwrap();
    tokio::time::sleep(Duration::from_secs(6)).await;

    // A new device signs in with the password and restores from the vault.
    let alice2 = Client::at("alice2", dir());
    let (u, number) = (server(), a.number.clone());
    alice2.core.run(move |i| async move { i.login(&u, &number, PASSWORD, "second", None).await }).await.unwrap();
    alice2.wait_online().await;
    alice2.wait("history restored", |n, v| n == "history_sync" && v["done"] == true).await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let contacts = alice2.core.run(|i| async move { i.list_contacts() }).await.unwrap();
    let bobby = contacts.iter().find(|x| x.account_id == b.account_id);
    assert_eq!(bobby.and_then(|x| x.local_name.as_deref()), Some("Bobby"), "the name Alice gave Bob was not restored: {contacts:?}");
    let id = c.account_id.clone();
    assert!(alice2.core.run(move |i| async move { Ok(i.is_blocked(&id)) }).await.unwrap(), "Carol is no longer blocked on the restored device");
}

async fn conv_of(c: &Client, peer: &str) -> String {
    let id = peer.to_string();
    c.core.run(move |i| async move { i.open_direct_conversation(&id).await }).await.unwrap().id
}
