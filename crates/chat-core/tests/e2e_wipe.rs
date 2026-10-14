//! "Delete for everyone" of a direct conversation removes it on the peer's devices and on all of
//! our own devices, whichever one deleted it.
//!
//! `CHATAPP_E2E_SERVER=http://127.0.0.1:8081 cargo test -p chat-core --test e2e_wipe`

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
async fn delete_for_everyone_reaches_our_other_devices() {
    if server().is_empty() {
        eprintln!("CHATAPP_E2E_SERVER not set, skipping");
        return;
    }
    let (phone, bob, laptop) = (Client::new(), Client::new(), Client::new());
    let mut accounts = Vec::new();
    for c in [&phone, &bob] {
        let u = server();
        let r = c.core.run(move |i| async move { i.register(&u, "wipe test password long enough", "test").await }).await.unwrap();
        c.wait("online", |n, v| n == "connection" && v.as_str() == Some("online")).await;
        accounts.push(r.account);
    }
    // The same account on a second device.
    let (u, num) = (server(), accounts[0].number.clone());
    laptop.core.run(move |i| async move { i.login(&u, &num, "wipe test password long enough", "laptop", None).await }).await.unwrap();
    laptop.wait("online", |n, v| n == "connection" && v.as_str() == Some("online")).await;

    let (bnum, bid) = (accounts[1].number.clone(), accounts[1].account_id.clone());
    phone.core.run(move |i| async move { i.add_contact_by_number(&bnum, Some("Bob".into())).await }).await.unwrap();
    let conv = phone.core.run(move |i| async move { i.open_direct_conversation(&bid).await }).await.unwrap().id;
    send(&phone, &conv, "hello from the phone").await;
    let on_bob = bob.wait("message on bob", |n, v| n == "message" && v["message"]["text"] == "hello from the phone").await;
    let on_laptop = laptop.wait("sync copy on the laptop", |n, v| n == "message" && v["message"]["text"] == "hello from the phone").await;
    let laptop_conv = on_laptop["message"]["conversation_id"].as_str().unwrap().to_string();
    let bob_conv = on_bob["message"]["conversation_id"].as_str().unwrap().to_string();

    // Deleted for everyone on the phone: gone on Bob's device and on our laptop.
    let c = conv.clone();
    phone.core.run(move |i| async move { i.delete_conversation_for_everyone(&c) }).await.unwrap();
    let lc = laptop_conv.clone();
    laptop.wait("conversation removed on the laptop", move |n, v| n == "conversation_removed" && v["conversation_id"] == lc.as_str()).await;
    let bc = bob_conv.clone();
    bob.wait("conversation removed on bob", move |n, v| n == "conversation_removed" && v["conversation_id"] == bc.as_str()).await;
    let lc = laptop_conv.clone();
    assert!(laptop.core.run(move |i| async move { i.get_conversation(&lc) }).await.is_err(), "still on the laptop");
}
