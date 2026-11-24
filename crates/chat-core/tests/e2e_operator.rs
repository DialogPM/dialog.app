//! Statements signed by the operator (engine/operator.rs): the badge of an official account
//! and notices. The honest path goes through the admin API, signed with the test admin key
//! that scripts/test-e2e.sh gives the server (seed bytes 0..31, as server-rs/tools/admin_smoke.py);
//! the attacks are played through the server's MySQL database (as tests/evil_server*.rs): a
//! server that wants to make up a badge or a notice can write any row it likes, but not the
//! operator's signature.
//!
//! `CHATAPP_E2E_SERVER=http://127.0.0.1:8080 CHATAPP_E2E_MYSQL="-uchatapp -pchatapp chatapp_dev"
//!  cargo test -p chat-core --test e2e_operator -- --nocapture`
//! (the server with CHATAPP_ADMIN_PUBLIC_KEYS=A6EHv/POEL4dcN0Y50vAmWfk1jCbpQ1fHdyGZBJVMbg=)

use std::sync::{Arc, Mutex};
use std::time::Duration;

use chat_core::crypto::{b64, unb64};
use chat_core::model::*;
use chat_core::{Core, CoreConfig, EventSink};
use ed25519_dalek::{Signer, SigningKey};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const DB_KEY: [u8; 32] = [7u8; 32];
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
    fn new(name: &'static str) -> Self {
        Self::at(name, Arc::new(tempfile::tempdir().unwrap()))
    }

    fn at(name: &'static str, dir: Arc<tempfile::TempDir>) -> Self {
        let events = Arc::new(Events::default());
        let core = Core::start(CoreConfig {
            data_dir: dir.path().to_path_buf(),
            db_key: DB_KEY,
            default_server_url: server(),
            platform: "cli".into(),
            events: events.clone(),
        })
        .unwrap();
        Self { name, core, events, dir }
    }

    async fn stop(self) -> (&'static str, Arc<tempfile::TempDir>) {
        self.core.run(|i| async move { i.stop_background(); Ok(()) }).await.unwrap();
        let (name, dir) = (self.name, self.dir.clone());
        drop(self);
        tokio::time::sleep(Duration::from_millis(800)).await;
        (name, dir)
    }

    async fn start_again((name, dir): (&'static str, Arc<tempfile::TempDir>)) -> Self {
        let c = Self::at(name, dir);
        if c.core.run(|i| async move { Ok(i.app_state().status) }).await.unwrap() == "locked" {
            c.core.run(|i| async move { i.unlock(PASSWORD).await }).await.unwrap();
        }
        c.wait_online().await;
        c
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
        self.wait_event(&format!("message '{text}'"), move |n, v| {
            n == "message" && v["message"]["text"].as_str() == Some(t.as_str())
        })
        .await["message"]
            .clone()
    }

    async fn wait_online(&self) {
        self.wait_event("online", |n, v| n == "connection" && v.as_str() == Some("online")).await;
    }

    async fn wait_notice(&self, title: &str) -> Value {
        let t = title.to_string();
        self.wait_event(&format!("notice '{title}'"), move |n, v| n == "notice" && v["title"].as_str() == Some(t.as_str())).await
    }

    fn saw_notice(&self, title: &str) -> bool {
        self.events.0.lock().unwrap().iter().any(|(n, v)| n == "notice" && v["title"].as_str() == Some(title))
    }

    /// A command as the UIs call it (rpc.rs).
    async fn rpc(&self, cmd: &str, req: Value) -> Value {
        chat_core::rpc::dispatch(&self.core, cmd, req).await.unwrap()
    }

    async fn notice_titles(&self) -> Vec<String> {
        let list = self.rpc("list_notices", json!({})).await;
        list["notices"].as_array().unwrap().iter().map(|n| n["title"].as_str().unwrap().to_string()).collect()
    }

    /// Id of the newest notice this client fetched from the server.
    async fn notice_cursor(&self) -> i64 {
        self.core
            .run(|i| async move { i.db.borrow().kv_get_json::<i64>("notices_after") })
            .await
            .unwrap()
            .unwrap_or(0)
    }

    async fn wait_cursor(&self, at_least: i64) {
        for _ in 0..300 {
            if self.notice_cursor().await >= at_least {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("{}: the notices were not fetched up to {at_least}", self.name);
    }

    async fn contact(&self, account_id: &str) -> Contact {
        let id = account_id.to_string();
        self.core.run(move |i| async move { i.contact_model(&id) }).await.unwrap()
    }

    async fn find(&self, number: &str) -> FoundAccount {
        let n = number.to_string();
        self.core.run(move |i| async move { i.find_account(&n).await }).await.unwrap().expect("account found")
    }

    async fn fetch_profile(&self, account_id: &str) {
        let id = account_id.to_string();
        self.core.run(move |i| async move { i.fetch_profile(&id).await }).await.unwrap();
    }
}

fn server() -> String {
    std::env::var("CHATAPP_E2E_SERVER").unwrap_or_default()
}

fn skip() -> bool {
    if server().is_empty() || std::env::var("CHATAPP_E2E_MYSQL").is_err() {
        eprintln!("CHATAPP_E2E_SERVER / CHATAPP_E2E_MYSQL not set, skipping");
        return true;
    }
    let _ = env_logger::builder().is_test(true).try_init();
    false
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

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64
}

/// A short random tag, so that this run's titles are told apart from other tests' notices
/// (notices for everyone reach every account of the test server).
fn tag() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..8].to_string()
}

/// The admin key the server is configured with (scripts/test-e2e.sh).
fn operator_key() -> SigningKey {
    SigningKey::from_bytes(&std::array::from_fn(|i| i as u8))
}

/// A key the server does not know and the apps did not pin.
fn other_key() -> SigningKey {
    SigningKey::from_bytes(&[9u8; 32])
}

fn badge_string(account_id: &str, identity_key: &[u8], issued_at: i64, label: &str) -> String {
    format!("dialog-operator-badge-v1\n{account_id}\n{}\n{issued_at}\n{label}", b64(identity_key))
}

fn notice_string(target: &str, created_at: i64, title: &str, body: &str) -> String {
    format!(
        "dialog-operator-notice-v1\n{target}\n{created_at}\n{}\n{}",
        hex::encode(Sha256::digest(title.as_bytes())),
        hex::encode(Sha256::digest(body.as_bytes()))
    )
}

/// A request to the admin API, signed like the admin app signs it (docs/ADMIN-API.md).
async fn admin(method: &str, path: &str, body: Option<Value>) -> Value {
    let body = body.map(|b| b.to_string().into_bytes()).unwrap_or_default();
    let ts = now_ms().to_string();
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let message = format!("dialog-admin-v1\n{method}\n{path}\n{ts}\n{nonce}\n{}", hex::encode(Sha256::digest(&body)));
    let key = operator_key();
    let r = reqwest::Client::new()
        .request(reqwest::Method::from_bytes(method.as_bytes()).unwrap(), format!("{}{path}", server()))
        .header("X-Admin-Key", b64(key.verifying_key().to_bytes()))
        .header("X-Admin-Timestamp", &ts)
        .header("X-Admin-Nonce", &nonce)
        .header("X-Admin-Signature", b64(key.sign(message.as_bytes()).to_bytes()))
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .await
        .unwrap();
    let status = r.status();
    let text = r.text().await.unwrap();
    assert!(status.is_success(), "admin {method} {path}: {status} {text}");
    if text.is_empty() { Value::Null } else { serde_json::from_str(&text).unwrap() }
}

/// The identity key the server has for an account (what the admin app signs a badge over).
fn server_identity_key(account_id: &str) -> Vec<u8> {
    unb64(&sql(&format!("SELECT TO_BASE64(identity_key) FROM accounts WHERE id = '{account_id}'")).replace('\n', "")).unwrap()
}

/// The operator marks an account as official through the admin API.
async fn grant_badge(account_id: &str, label: &str) {
    let issued_at = now_ms();
    let signed = badge_string(account_id, &server_identity_key(account_id), issued_at, label);
    let key = operator_key();
    admin(
        "POST",
        &format!("/v1/admin/accounts/{account_id}/badge"),
        Some(json!({ "label": label, "issued_at": issued_at, "key": b64(key.verifying_key().to_bytes()),
                     "signature": b64(key.sign(signed.as_bytes()).to_bytes()) })),
    )
    .await;
}

/// The operator publishes a notice (`target`: "*" or an account id); returns its id.
async fn publish_notice(target: &str, title: &str, body: &str) -> i64 {
    let created_at = now_ms();
    let key = operator_key();
    let signature = key.sign(notice_string(target, created_at, title, body).as_bytes()).to_bytes();
    let r = admin(
        "POST",
        "/v1/admin/notices",
        Some(json!({ "target": target, "title": title, "body": body, "created_at": created_at,
                     "key": b64(key.verifying_key().to_bytes()), "signature": b64(signature) })),
    )
    .await;
    r["id"].as_i64().unwrap()
}

/// A notice row written straight into the server's database: `stored_*` is what the row holds,
/// the signature is `signer`'s over `signed_target` and the signed title and body.
#[allow(clippy::too_many_arguments)]
fn insert_notice(
    stored_account: Option<&str>,
    stored_title: &str,
    stored_body: &str,
    signer: &SigningKey,
    signed_target: &str,
    signed_title: &str,
    signed_body: &str,
) -> i64 {
    let created_at = now_ms();
    let signature = signer.sign(notice_string(signed_target, created_at, signed_title, signed_body).as_bytes()).to_bytes();
    let account = stored_account.map_or("NULL".to_string(), |a| format!("'{a}'"));
    sql(&format!(
        "INSERT INTO operator_notices (account_id, title, body, created_at, signer, signature) VALUES \
         ({account}, '{stored_title}', '{stored_body}', {created_at}, UNHEX('{}'), UNHEX('{}')); SELECT LAST_INSERT_ID();",
        hex::encode(signer.verifying_key().to_bytes()),
        hex::encode(signature)
    ))
    .parse()
    .unwrap()
}

async fn register(clients: &[&Client]) -> Vec<AccountInfo> {
    let mut accounts = Vec::new();
    for c in clients {
        let u = server();
        let r = c.core.run(move |i| async move { i.register(&u, PASSWORD, "test device").await }).await.unwrap();
        c.wait_online().await;
        accounts.push(r.account);
    }
    accounts
}

/// Waits until a client pinned the operator keys (at its first connection).
async fn wait_pinned(c: &Client) -> Vec<String> {
    for _ in 0..100 {
        let keys = c.core.run(|i| async move { i.db.borrow().kv_get_json::<Vec<String>>("operator_keys") }).await.unwrap();
        if let Some(k) = keys {
            return k;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("{}: the operator keys were not pinned", c.name);
}

/// The operator marks Alice as official: Bob, who talks with her, sees the badge on her
/// contact and their conversation once he refreshes her profile, and loses it when the badge
/// is removed. Alice sees her own badge on her profile.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_official_badge_follows_the_profile() {
    if skip() {
        return;
    }
    let alice = Client::new("alice");
    let bob = Client::new("bob");
    let acc = register(&[&alice, &bob]).await;
    let (a, b) = (acc[0].clone(), acc[1].clone());
    assert_eq!(wait_pinned(&alice).await, vec![b64(operator_key().verifying_key().to_bytes())]);
    let req: SetProfileRequest = serde_json::from_value(json!({ "name": "Support" })).unwrap();
    alice.core.run(move |i| async move { i.set_profile(req).await }).await.unwrap();

    // Bob writes to Alice, she accepts: Bob gets her profile key (and pinned her identity).
    let (anum, aid) = (a.number.clone(), a.account_id.clone());
    bob.core.run(move |i| async move { i.add_contact_by_number(&anum, None).await }).await.unwrap();
    let bob_conv = bob.core.run(move |i| async move { i.open_direct_conversation(&aid).await }).await.unwrap().id;
    let req: SendMessageRequest = serde_json::from_value(json!({ "conversation_id": bob_conv, "text": "hello support" })).unwrap();
    bob.core.run(move |i| async move { i.send_message(req).await }).await.unwrap();
    let alice_conv = alice.wait_message("hello support").await["conversation_id"].as_str().unwrap().to_string();
    alice.core.run(move |i| async move { i.accept_conversation(&alice_conv).await }).await.unwrap();
    let aid = a.account_id.clone();
    bob.wait_event("alice's profile", move |n, v| {
        n == "contact_updated" && v["account_id"].as_str() == Some(aid.as_str()) && v["profile_name"].as_str() == Some("Support")
    })
    .await;
    assert_eq!(bob.contact(&a.account_id).await.official, None);

    grant_badge(&a.account_id, "Dialog Support").await;
    bob.fetch_profile(&a.account_id).await;
    assert_eq!(bob.contact(&a.account_id).await.official.as_deref(), Some("Dialog Support"));
    let bc = bob_conv.clone();
    let conv = bob.core.run(move |i| async move { i.get_conversation(&bc) }).await.unwrap();
    assert_eq!(conv.official.as_deref(), Some("Dialog Support"));
    let aid = a.account_id.clone();
    bob.wait_event("the badge announced", move |n, v| {
        n == "contact_updated" && v["account_id"].as_str() == Some(aid.as_str()) && v["official"].as_str() == Some("Dialog Support")
    })
    .await;
    bob.wait_event("the conversation announced", |n, v| n == "conversation_updated" && v["official"].as_str() == Some("Dialog Support"))
        .await;
    // Through the RPC layer as the UIs read it, and in the directory search.
    let c = bob.rpc("get_contact", json!({ "account_id": a.account_id })).await;
    assert_eq!(c["official"], "Dialog Support");
    assert_eq!(bob.find(&a.number).await.official.as_deref(), Some("Dialog Support"));
    // Alice's own badge.
    alice.fetch_profile(&a.account_id).await;
    assert_eq!(alice.rpc("get_profile", json!({})).await["official"], "Dialog Support");
    // Editing the profile keeps it, in the reply and in the event the UIs store.
    assert_eq!(alice.rpc("set_profile", json!({ "about": "We answer." })).await["official"], "Dialog Support");
    alice
        .wait_event("profile event with the badge", |n, v| {
            n == "profile_updated" && v["about"] == "We answer." && v["official"] == "Dialog Support"
        })
        .await;
    // Not Bob: nobody signed anything about him.
    assert_eq!(alice.find(&b.number).await.official, None);

    admin("POST", &format!("/v1/admin/accounts/{}/unbadge", a.account_id), None).await;
    bob.fetch_profile(&a.account_id).await;
    assert_eq!(bob.contact(&a.account_id).await.official, None);
    let bc = bob_conv.clone();
    assert_eq!(bob.core.run(move |i| async move { i.get_conversation(&bc) }).await.unwrap().official, None);
    alice.fetch_profile(&a.account_id).await;
    assert_eq!(alice.rpc("get_profile", json!({})).await["official"], Value::Null);
}

/// A server cannot make an account official by itself: a badge signed by a key the apps did
/// not pin, a valid badge copied to another account, and a badge left in place after the
/// account's identity key was replaced are all refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_badge_the_server_makes_up_is_not_shown() {
    if skip() {
        return;
    }
    let alice = Client::new("alice");
    let carol = Client::new("carol");
    let dave = Client::new("dave");
    let eve = Client::new("eve");
    let acc = register(&[&alice, &carol, &dave, &eve]).await;
    let (a, c, d) = (acc[0].clone(), acc[1].clone(), acc[2].clone());
    wait_pinned(&eve).await;

    // The honest badge, as a control: Eve (who has no key of Alice pinned) sees it.
    grant_badge(&a.account_id, "Dialog Support").await;
    assert_eq!(eve.find(&a.number).await.official.as_deref(), Some("Dialog Support"));

    // 1. Signed by a key the server holds (it could), not by a pinned operator key.
    let issued_at = now_ms();
    let signed = badge_string(&c.account_id, &server_identity_key(&c.account_id), issued_at, "Dialog Support");
    sql(&format!(
        "INSERT INTO operator_badges (account_id, label, issued_at, signer, signature) VALUES ('{}', 'Dialog Support', {issued_at}, UNHEX('{}'), UNHEX('{}'))",
        c.account_id,
        hex::encode(other_key().verifying_key().to_bytes()),
        hex::encode(other_key().sign(signed.as_bytes()).to_bytes())
    ));
    // Even presented under the name of the operator's key.
    let found = eve.find(&c.number).await;
    assert_eq!(found.official, None, "a badge signed by an unpinned key was shown");
    sql(&format!(
        "UPDATE operator_badges SET signer = UNHEX('{}') WHERE account_id = '{}'",
        hex::encode(operator_key().verifying_key().to_bytes()),
        c.account_id
    ));
    assert_eq!(eve.find(&c.number).await.official, None, "a badge with a forged signer was shown");

    // 2. Alice's valid badge copied to Dave.
    sql(&format!(
        "INSERT INTO operator_badges (account_id, label, issued_at, signer, signature)
         SELECT '{}', label, issued_at, signer, signature FROM operator_badges WHERE account_id = '{}'",
        d.account_id, a.account_id
    ));
    assert_eq!(eve.find(&d.number).await.official, None, "a badge copied to another account was shown");
    // Through the lookup by account id too (a first message from Dave does this), and when
    // he is added as a contact.
    let did = d.account_id.clone();
    eve.core.run(move |i| async move { i.ensure_contact(&did).await }).await.unwrap();
    assert_eq!(eve.contact(&d.account_id).await.official, None);
    let dnum = d.number.clone();
    let contact = eve.core.run(move |i| async move { i.add_contact_by_number(&dnum, None).await }).await.unwrap();
    assert_eq!(contact.official, None);

    // 3. Alice's identity key replaced by another (Carol's), the badge left in place: it was
    // signed over her old key, and whoever looks her up now gets the new one.
    sql(&format!(
        "UPDATE accounts SET identity_key = (SELECT k FROM (SELECT identity_key AS k FROM accounts WHERE id = '{}') x) WHERE id = '{}'",
        c.account_id, a.account_id
    ));
    assert_eq!(eve.find(&a.number).await.official, None, "a badge over a replaced identity key was shown");
}

/// Notices for everyone and for one account arrive (pushed by the server's event), are listed
/// newest first with the unread count, and can be marked read and deleted.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn notices_from_the_operator_arrive() {
    if skip() {
        return;
    }
    let alice = Client::new("alice");
    let bob = Client::new("bob");
    let acc = register(&[&alice, &bob]).await;
    let a = acc[0].clone();
    wait_pinned(&alice).await;
    wait_pinned(&bob).await;
    let t = tag();
    let everyone = format!("Maintenance {t}");
    let personal = format!("For Alice {t}");

    let id1 = publish_notice("*", &everyone, "Tonight at 23:00.\nAbout an hour.").await;
    let n = alice.wait_notice(&everyone).await;
    assert_eq!(n["id"].as_i64(), Some(id1));
    assert_eq!(n["body"], "Tonight at 23:00.\nAbout an hour.");
    assert_eq!(n["read"], false);
    assert!(n["created_at"].as_i64().unwrap() > 0 && n["received_at"].as_i64().unwrap() > 0);
    bob.wait_notice(&everyone).await;

    let id2 = publish_notice(&a.account_id, &personal, "Only for you.").await;
    alice.wait_notice(&personal).await;
    alice.wait_event("unread count", |n, v| n == "notices_changed" && v["unread"].as_i64().is_some_and(|u| u >= 2)).await;
    bob.wait_cursor(id1).await;
    assert!(!bob.saw_notice(&personal) && !bob.notice_titles().await.contains(&personal));

    let list = alice.rpc("list_notices", json!({})).await;
    let notices = list["notices"].as_array().unwrap();
    let ids: Vec<i64> = notices.iter().map(|n| n["id"].as_i64().unwrap()).collect();
    assert!(ids.windows(2).all(|w| w[0] > w[1]), "newest first: {ids:?}");
    assert!(ids.contains(&id1) && ids.contains(&id2));
    let unread = notices.iter().filter(|n| n["read"] == false).count() as i64;
    assert!(unread >= 2);
    assert_eq!(list["unread"].as_i64(), Some(unread));

    assert_eq!(alice.rpc("mark_notices_read", json!({})).await, json!({ "unread": 0 }));
    let list = alice.rpc("list_notices", json!({})).await;
    assert_eq!(list["unread"], 0);
    assert!(list["notices"].as_array().unwrap().iter().all(|n| n["read"] == true));
    alice.wait_event("all read", |n, v| n == "notices_changed" && v["unread"] == 0).await;

    assert_eq!(alice.rpc("delete_notice", json!({ "id": id2 })).await, json!({}));
    let titles = alice.notice_titles().await;
    assert!(titles.contains(&everyone) && !titles.contains(&personal));

    // Kept across a restart; a deleted one does not come back.
    let alice = Client::start_again(alice.stop().await).await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let titles = alice.notice_titles().await;
    assert!(titles.contains(&everyone) && !titles.contains(&personal));
}

/// Notices the server makes up or alters are not shown: text edited after signing, a notice
/// signed by a key that is not pinned, and another account's notice served to us.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_notice_the_server_makes_up_is_not_shown() {
    if skip() {
        return;
    }
    let alice = Client::new("alice");
    let bob = Client::new("bob");
    let acc = register(&[&alice, &bob]).await;
    let (a, b) = (acc[0].clone(), acc[1].clone());
    wait_pinned(&alice).await;
    let t = tag();
    let op = operator_key();

    let edited = format!("Edited {t}");
    insert_notice(None, &edited, "Send your recovery phrase to support", &op, "*", &edited, "Nothing to do.");
    let edited_title = format!("Retitled {t}");
    insert_notice(None, &edited_title, "Nothing to do.", &op, "*", &format!("Original {t}"), "Nothing to do.");
    let unpinned = format!("Unpinned {t}");
    insert_notice(Some(&a.account_id), &unpinned, "Hello.", &other_key(), &a.account_id, &unpinned, "Hello.");
    let unpinned_named = format!("Unpinned named {t}");
    let forged = insert_notice(Some(&a.account_id), &unpinned_named, "Hello.", &other_key(), &a.account_id, &unpinned_named, "Hello.");
    sql(&format!("UPDATE operator_notices SET signer = UNHEX('{}') WHERE id = {forged}", hex::encode(op.verifying_key().to_bytes())));
    let theirs = format!("For Bob {t}");
    insert_notice(Some(&a.account_id), &theirs, "Only for Bob.", &op, &b.account_id, &theirs, "Only for Bob.");
    // A genuine one after them, published normally: its event makes Alice fetch them all.
    let genuine = format!("Genuine {t}");
    let id = publish_notice(&a.account_id, &genuine, "Real.").await;
    alice.wait_notice(&genuine).await;
    alice.wait_cursor(id).await;

    let titles = alice.notice_titles().await;
    for forged in [&edited, &edited_title, &unpinned, &unpinned_named, &theirs] {
        assert!(!titles.contains(forged) && !alice.saw_notice(forged), "a forged notice was shown: {forged}");
    }
    assert!(titles.contains(&genuine));
    // Skipped, not retried: a restart does not bring them.
    let alice = Client::start_again(alice.stop().await).await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let titles = alice.notice_titles().await;
    assert!(titles.contains(&genuine) && !titles.contains(&edited) && !titles.contains(&theirs));
}

/// The operator keys are pinned at first contact: a server that later announces other keys
/// (to sign badges and notices of its own) does not get them adopted.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_operator_keys_are_pinned() {
    if skip() {
        return;
    }
    let alice = Client::new("alice");
    let a = register(&[&alice]).await[0].clone();
    let served = wait_pinned(&alice).await;
    assert_eq!(served, vec![b64(operator_key().verifying_key().to_bytes())]);

    // The pin is now another key than the one the server announces: from the client's point
    // of view, the server's keys changed since they were pinned.
    let pinned = vec![b64(other_key().verifying_key().to_bytes())];
    let p = pinned.clone();
    alice.core.run(move |i| async move { i.db.borrow().kv_set_json("operator_keys", &p) }).await.unwrap();
    let alice = Client::start_again(alice.stop().await).await;
    for _ in 0..30 {
        let keys = alice.core.run(|i| async move { i.db.borrow().kv_get_json::<Vec<String>>("operator_keys") }).await.unwrap();
        assert_eq!(keys.as_ref(), Some(&pinned), "the server replaced the pinned operator keys");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // What the pinned key signed is shown; what the server's announced key signed is not.
    let t = tag();
    let by_pinned = format!("Pinned key {t}");
    insert_notice(Some(&a.account_id), &by_pinned, "Hello.", &other_key(), &a.account_id, &by_pinned, "Hello.");
    let by_announced = format!("Announced key {t}");
    let id = publish_notice(&a.account_id, &by_announced, "Hello.").await;
    alice.wait_notice(&by_pinned).await;
    alice.wait_cursor(id).await;
    assert!(!alice.notice_titles().await.contains(&by_announced) && !alice.saw_notice(&by_announced));

    // A badge too: the operator's real key is not pinned here.
    grant_badge(&a.account_id, "Dialog Support").await;
    alice.fetch_profile(&a.account_id).await;
    assert_eq!(alice.rpc("get_profile", json!({})).await["official"], Value::Null);
}
