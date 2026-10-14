//! End-to-end tests of usernames with discriminators (`name.01`) and username links
//! (engine/username_link.rs, server-rs api/username.rs).
//!
//! `CHATAPP_E2E_SERVER=http://127.0.0.1:8099 cargo test -p chat-core --test e2e_username_link`

use std::sync::{Arc, Mutex};
use std::time::Duration;

use chat_core::model::*;
use chat_core::{Core, CoreConfig, EventSink};
use serde_json::Value;

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
            if let Some((_, v)) = self.events.0.lock().unwrap().iter().rev().find(|(n, v)| pred(n, v)) {
                return v.clone();
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("{}: timed out waiting for {what}", self.name);
    }

    async fn wait_online(&self) {
        self.wait_event("online", |n, v| n == "connection" && v.as_str() == Some("online")).await;
    }

    async fn register(&self, device: &'static str) -> AccountInfo {
        let u = server();
        let r = self.core.run(move |i| async move { i.register(&u, PASSWORD, device).await }).await.unwrap();
        self.wait_online().await;
        r.account
    }

}

fn server() -> String {
    std::env::var("CHATAPP_E2E_SERVER").unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn discriminators_and_username_links() {
    if server().is_empty() {
        eprintln!("CHATAPP_E2E_SERVER not set, skipping");
        return;
    }
    let (ana, bob) = (Client::new("ana"), Client::new("bob"));
    let ana_acc = ana.register("ana").await;
    bob.register("bob").await;
    let nick = format!("n{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);

    // No username: no link.
    assert!(ana.core.run(|i| async move { i.get_username_link().await }).await.unwrap().is_none());

    // Plain usernames still work; discriminators follow Signal's rules.
    for bad in ["00", "1", "012", "1234567890"] {
        let u = format!("{nick}.{bad}");
        let err = ana.core.run(move |i| async move { i.set_username(Some(u)).await }).await.unwrap_err();
        assert_eq!(err.code.as_str(), "invalid_input", "{bad}");
    }
    let plain = nick.clone();
    ana.core.run(move |i| async move { i.set_username(Some(plain)).await }).await.unwrap();
    let full = format!("{nick}.07");
    let f = full.clone();
    ana.core.run(move |i| async move { i.set_username(Some(f)).await }).await.unwrap();
    // The same nickname with another discriminator is a different username.
    let other = format!("{nick}.08");
    let o = other.clone();
    bob.core.run(move |i| async move { i.set_username(Some(o)).await }).await.unwrap();
    let f = full.clone();
    let found = bob.core.run(move |i| async move { i.find_account(&f).await }).await.unwrap().unwrap();
    assert_eq!(found.account_id, ana_acc.account_id);

    // Username link: no account number inside, stable until reset.
    let link = ana.core.run(|i| async move { i.get_username_link().await }).await.unwrap().unwrap();
    assert_eq!(link.username, full);
    assert!(link.link.starts_with("chatapp://u?"), "{}", link.link);
    assert!(!link.link.contains(&ana_acc.number));
    assert!(!link.link.contains(&full));
    let again = ana.core.run(|i| async move { i.get_username_link().await }).await.unwrap().unwrap();
    assert_eq!(again.link, link.link);

    let l = link.link.clone();
    let found = bob.core.run(move |i| async move { i.resolve_username_link(&l).await }).await.unwrap();
    assert_eq!(found.account_id, ana_acc.account_id);
    assert_eq!(found.username.as_deref(), Some(full.as_str()));
    let l = link.link.clone();
    let c = bob.core.run(move |i| async move { i.add_contact_from_link(&l).await }).await.unwrap();
    assert_eq!(c.account_id, ana_acc.account_id);

    // A tampered key does not decrypt.
    let (base, frag) = link.link.split_once('#').unwrap();
    let mut chars: Vec<char> = frag.chars().collect();
    chars[0] = if chars[0] == 'A' { 'B' } else { 'A' };
    let bad = format!("{base}#{}", chars.into_iter().collect::<String>());
    assert!(bob.core.run(move |i| async move { i.resolve_username_link(&bad).await }).await.is_err());

    // Reset: the old link stops working, the new one works.
    let reset = ana.core.run(|i| async move { i.reset_username_link().await }).await.unwrap().unwrap();
    assert_ne!(reset.link, link.link);
    let l = link.link.clone();
    let err = bob.core.run(move |i| async move { i.resolve_username_link(&l).await }).await.unwrap_err();
    assert_eq!(err.code.as_str(), "not_found");
    let l = reset.link.clone();
    bob.core.run(move |i| async move { i.resolve_username_link(&l).await }).await.unwrap();

    // Changing the username invalidates the link; a new one is made on demand.
    let plain = nick.clone();
    ana.core.run(move |i| async move { i.set_username(Some(plain)).await }).await.unwrap();
    let l = reset.link.clone();
    assert!(bob.core.run(move |i| async move { i.resolve_username_link(&l).await }).await.is_err());
    let fresh = ana.core.run(|i| async move { i.get_username_link().await }).await.unwrap().unwrap();
    assert_eq!(fresh.username, nick);
    let l = fresh.link.clone();
    let found = bob.core.run(move |i| async move { i.resolve_username_link(&l).await }).await.unwrap();
    assert_eq!(found.username.as_deref(), Some(nick.as_str()));
}
