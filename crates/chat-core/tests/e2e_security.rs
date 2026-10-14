//! End-to-end tests of the account security features (engine/security.rs, server-rs
//! api/security.rs): new-device alerts, encrypted device names, approval of new logins,
//! spam reports, registration proof-of-work, group bans and username reservations.
//!
//! Run with a server listening (rate limits disabled), e.g.:
//! `CHATAPP_E2E_SERVER=http://127.0.0.1:8080 cargo test -p chat-core --test e2e_security`

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

    async fn login(&self, number: &str, device: &'static str) -> chat_core::Result<AccountInfo> {
        let (u, n) = (server(), number.to_string());
        self.core.run(move |i| async move { i.login(&u, &n, PASSWORD, device, None).await }).await
    }

    /// The device list as the server stores it (names not decrypted).
    async fn raw_devices(&self) -> Vec<Value> {
        let v: Value = self.core.run(|i| async move { i.session()?.api.get("/v1/devices").await }).await.unwrap();
        v["devices"].as_array().unwrap().clone()
    }
}

fn server() -> String {
    std::env::var("CHATAPP_E2E_SERVER").unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn new_device_alerts_and_encrypted_names() {
    if server().is_empty() {
        eprintln!("CHATAPP_E2E_SERVER not set, skipping");
        return;
    }
    let desk = Client::new("desk");
    let acc = desk.register("Ana desktop").await;
    // The first check after joining records the list without alerting.
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(desk.core.run(|i| async move { i.device_alerts() }).await.unwrap().is_empty());

    let phone = Client::new("phone");
    phone.login(&acc.number, "Ana phone").await.unwrap();
    phone.wait_online().await;

    // The existing device is alerted, with the new device's (decrypted) name.
    desk.wait_event("alert", |n, v| {
        n == "device_alerts" && v.as_array().is_some_and(|a| a.len() == 1 && a[0]["name"] == "Ana phone")
    })
    .await;
    // Not the new device itself.
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(phone.core.run(|i| async move { i.device_alerts() }).await.unwrap().is_empty());

    // Names are readable by the account's devices, the server only has placeholders.
    let mut names: Vec<String> =
        desk.core.run(|i| async move { i.list_devices().await }).await.unwrap().into_iter().map(|d| d.name).collect();
    names.sort();
    assert_eq!(names, ["Ana desktop", "Ana phone"]);
    for d in desk.raw_devices().await {
        assert_eq!(d["name"], "Dialog", "{d}");
        assert!(d["encrypted_name"].as_str().is_some_and(|e| !e.is_empty()), "{d}");
    }
    phone.core.run(|i| async move { i.rename_device("Ana tablet").await }).await.unwrap();
    desk.wait_event("renamed", |n, v| n == "device_alerts" && v[0]["name"] == "Ana tablet").await;

    desk.core.run(|i| async move { i.dismiss_device_alerts() }).await.unwrap();
    assert!(desk.core.run(|i| async move { i.device_alerts() }).await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn login_approval() {
    if server().is_empty() {
        return;
    }
    let desk = Client::new("desk");
    let acc = desk.register("desk").await;
    desk.core.run(|i| async move { i.set_login_approval(true).await }).await.unwrap();
    assert!(desk.core.run(|i| async move { i.account_details().await }).await.unwrap().login_approval);

    // Approved.
    let laptop = Client::new("laptop");
    let err = laptop.login(&acc.number, "laptop").await.unwrap_err();
    assert_eq!(err.code.as_str(), "approval_required");
    let req = desk
        .wait_event("login request", |n, v| n == "login_requests" && v.as_array().is_some_and(|a| a.len() == 1))
        .await;
    assert_eq!(req[0]["name"], "laptop", "the approving device sees the real name");
    let id = req[0]["id"].as_str().unwrap().to_string();
    let waiter = laptop.core.clone();
    let wait = tokio::spawn(async move { waiter.run(|i| async move { i.login_wait_approval().await }).await });
    tokio::time::sleep(Duration::from_millis(500)).await;
    desk.core.run(move |i| async move { i.answer_login_request(&id, true).await }).await.unwrap();
    let got = wait.await.unwrap().unwrap();
    assert_eq!(got.account_id, acc.account_id);
    laptop.wait_online().await;

    // Denied.
    let tablet = Client::new("tablet");
    assert_eq!(tablet.login(&acc.number, "tablet").await.unwrap_err().code.as_str(), "approval_required");
    let reqs = desk.core.run(|i| async move { i.list_login_requests().await }).await.unwrap();
    assert_eq!(reqs.len(), 1);
    let id = reqs[0].id.clone();
    desk.core.run(move |i| async move { i.answer_login_request(&id, false).await }).await.unwrap();
    let err = tablet.core.run(|i| async move { i.login_wait_approval().await }).await.unwrap_err();
    assert_eq!(err.code.as_str(), "login_denied");

    // Cancelled by the user.
    let phone = Client::new("phone");
    assert_eq!(phone.login(&acc.number, "phone").await.unwrap_err().code.as_str(), "approval_required");
    phone.core.run(|i| async move { i.login_cancel_approval(); Ok(()) }).await.unwrap();
    assert_eq!(phone.core.run(|i| async move { i.login_wait_approval().await }).await.unwrap_err().code.as_str(), "gone");

    // Turned off: logins go through again.
    desk.core.run(|i| async move { i.set_login_approval(false).await }).await.unwrap();
    let watch = Client::new("watch");
    watch.login(&acc.number, "watch").await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn spam_report_and_group_ban() {
    use chat_core::engine::invites::SetInviteRequest;
    if server().is_empty() {
        return;
    }
    let (ana, bob) = (Client::new("ana"), Client::new("bob"));
    let a = ana.register("ana").await;
    let b = bob.register("bob").await;

    // Report (and block).
    let a_id = a.account_id.clone();
    let c = bob.core.run(move |i| async move { i.report_spam(&a_id, true).await }).await.unwrap();
    assert!(c.blocked);
    let me = b.account_id.clone();
    assert!(bob.core.run(move |i| async move { i.report_spam(&me, false).await }).await.is_err());

    // A member removed by an admin cannot come back by link until re-added.
    let req: CreateGroupRequest =
        serde_json::from_value(serde_json::json!({ "name": "Ban club", "member_account_ids": [b.account_id] })).unwrap();
    let g = ana.core.run(move |i| async move { i.create_group(req).await }).await.unwrap();
    let (gid, bid) = (g.id.clone(), b.account_id.clone());
    ana.core.run(move |i| async move { i.remove_group_member(&gid, &bid).await }).await.unwrap();
    let gid = g.id.clone();
    let info = ana
        .core
        .run(move |i| async move {
            i.set_group_invite(SetInviteRequest { conversation_id: gid, enabled: true, approval: Some(false), reset: false }).await
        })
        .await
        .unwrap();
    let link = info.invite_link.unwrap();
    let l = link.clone();
    let p = bob.core.run(move |i| async move { i.preview_group_invite(&l).await }).await.unwrap();
    assert!(p.banned && !p.is_member);
    let l = link.clone();
    let err = bob.core.run(move |i| async move { i.join_group_by_link(&l).await }).await.unwrap_err();
    assert_eq!(err.code.as_str(), "banned", "a removed member gets the specific error");
    let (gid, bid) = (g.id.clone(), b.account_id.clone());
    ana.core.run(move |i| async move { i.add_group_members(&gid, vec![bid]).await }).await.unwrap();
    let l = link.clone();
    let p = bob.core.run(move |i| async move { i.preview_group_invite(&l).await }).await.unwrap();
    assert!(!p.banned && p.is_member);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn username_reservation() {
    if server().is_empty() {
        return;
    }
    let (ana, bob) = (Client::new("ana"), Client::new("bob"));
    ana.register("ana").await;
    bob.register("bob").await;
    let suffix: String = uuid::Uuid::new_v4().simple().to_string()[..10].to_string();
    let (old, new) = (format!("f{suffix}"), format!("g{suffix}"));
    let o = old.clone();
    ana.core.run(move |i| async move { i.set_username(Some(o)).await }).await.unwrap();
    let n = new.clone();
    ana.core.run(move |i| async move { i.set_username(Some(n)).await }).await.unwrap();
    // Released, but still reserved for ana.
    let o = old.clone();
    let err = bob.core.run(move |i| async move { i.set_username(Some(o)).await }).await.unwrap_err();
    assert_eq!(err.code.as_str(), "conflict");
    let o = old.clone();
    ana.core.run(move |i| async move { i.set_username(Some(o)).await }).await.unwrap();
    // Look-alike characters are refused rather than folded onto ASCII.
    let err = bob.core.run(|i| async move { i.set_username(Some("\u{212A}ate_x1".into())).await }).await.unwrap_err();
    assert_eq!(err.code.as_str(), "invalid_input");
}
