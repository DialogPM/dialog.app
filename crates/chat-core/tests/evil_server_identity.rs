//! Identities and sealed sender against a compromised server: who a message is from, which
//! key a contact has, and which keys vouch for senders must not be the server's to decide.
//! The tests play the attacker through the server's MySQL database (as tests/evil_server.rs)
//! and, where the attack needs one, through a modified client.
//!
//! `CHATAPP_E2E_SERVER=http://127.0.0.1:8080 CHATAPP_E2E_MYSQL="-uchatapp -pchatapp chatapp_dev"
//!  cargo test -p chat-core --test evil_server_identity -- --nocapture`

use std::sync::{Arc, Mutex};
use std::time::Duration;

use chat_core::model::*;
use chat_core::{Core, CoreConfig, EventSink};
use serde_json::{json, Value};

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

    /// Stops the engine (the app is closed); `start_again` brings it back on the same data.
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

    async fn wait_status(&self, id: &str, status: &str) {
        let (id, st) = (id.to_string(), status.to_string());
        self.wait_event(&format!("status {status}"), move |n, v| {
            n == "message" && v["message"]["id"].as_str() == Some(id.as_str()) && v["message"]["status"].as_str() == Some(st.as_str())
        })
        .await;
    }

    async fn wait_identity_conflict(&self, account_id: &str) {
        let id = account_id.to_string();
        self.wait_event("identity conflict", move |n, v| n == "identity_conflict" && v["account_id"].as_str() == Some(id.as_str()))
            .await;
    }

    async fn wait_kt_alert(&self, kind: &str) {
        let k = kind.to_string();
        self.wait_event(&format!("kt alert {kind}"), move |n, v| {
            n == "kt_alerts" && v.as_array().is_some_and(|a| a.iter().any(|x| x["kind"].as_str() == Some(k.as_str())))
        })
        .await;
    }

    fn saw_message(&self, text: &str) -> bool {
        self.events.0.lock().unwrap().iter().any(|(n, v)| n == "message" && v["message"]["text"].as_str() == Some(text))
    }

    async fn wait_online(&self) {
        self.wait_event("online", |n, v| n == "connection" && v.as_str() == Some("online")).await;
    }

    /// A value of this client's own key-value store.
    async fn kv(&self, key: &'static str) -> Option<Vec<u8>> {
        self.core.run(move |i| async move { i.db.borrow().kv_get(key) }).await.unwrap()
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

async fn send_text(c: &Client, conv: &str, text: &str) -> Message {
    let req: SendMessageRequest = serde_json::from_value(json!({ "conversation_id": conv, "text": text })).unwrap();
    c.core.run(move |i| async move { i.send_message(req).await }).await.unwrap()
}

/// `a` adds `b` as a contact and opens the chat; returns a's conversation id.
async fn open_chat(a: &Client, b: &AccountInfo) -> String {
    let (num, id) = (b.number.clone(), b.account_id.clone());
    a.core.run(move |i| async move { i.add_contact_by_number(&num, None).await }).await.unwrap();
    a.core.run(move |i| async move { i.open_direct_conversation(&id).await }).await.unwrap().id
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64
}

/// The attacker's first message to `to`, in the name of `as_account` (device `as_device`) but
/// under an identity key of its own: a modified client fetches `to`'s pre-key bundle (with
/// `fetcher`'s account) and encrypts a pre-key message from a fresh identity, presenting the
/// registration id of the impersonated device. The server then queues it as coming from
/// `source` (an account id it chooses). `to` must be offline (the envelope is written into
/// the queue directly).
async fn forge_first_message(fetcher: &Client, to: &AccountInfo, as_account: &str, as_device: u32, source: &str, text: &str) {
    use chat_core::api::BundlesDto;
    use chat_core::content::{Content, DataMessage};
    use libsignal_protocol::*;

    let (to_id, impersonated) = (to.account_id.clone(), as_account.to_string());
    let (bundles, theirs): (BundlesDto, BundlesDto) = fetcher
        .core
        .run(move |i| async move {
            let api = i.session()?.api.clone();
            Ok((api.get(&format!("/v1/keys/{to_id}/*")).await?, api.get(&format!("/v1/keys/{impersonated}/*")).await?))
        })
        .await
        .unwrap();
    let registration_id = theirs.devices.iter().find(|d| d.device_id == as_device).unwrap().registration_id;
    let unb64 = |s: &str| chat_core::crypto::unb64(s).unwrap();
    let identity = IdentityKey::decode(&unb64(&bundles.identity_key)).unwrap();
    let d = bundles.devices.iter().find(|d| d.device_id == to.device_id).unwrap();
    let bundle = PreKeyBundle::new(
        d.registration_id,
        DeviceId::new(d.device_id as u8).unwrap(),
        d.prekey.as_ref().map(|p| (PreKeyId::from(p.id), PublicKey::deserialize(&unb64(&p.public_key)).unwrap())),
        SignedPreKeyId::from(d.signed_prekey.id),
        PublicKey::deserialize(&unb64(&d.signed_prekey.public_key)).unwrap(),
        unb64(&d.signed_prekey.signature),
        KyberPreKeyId::from(d.kyber_prekey.id),
        kem::PublicKey::deserialize(&unb64(&d.kyber_prekey.public_key)).unwrap(),
        unb64(&d.kyber_prekey.signature),
        identity,
    )
    .unwrap();
    let mut rng = rand::rng();
    let mut store = InMemSignalProtocolStore::new(IdentityKeyPair::generate(&mut rng), registration_id).unwrap();
    let remote = ProtocolAddress::new(to.account_id.clone(), DeviceId::new(to.device_id as u8).unwrap());
    let local = ProtocolAddress::new(as_account.to_string(), DeviceId::new(as_device as u8).unwrap());
    let now = std::time::SystemTime::now();
    process_prekey_bundle(&remote, &local, &mut store.session_store, &mut store.identity_store, &bundle, now, &mut rng)
        .await
        .unwrap();
    let dm = DataMessage { id: uuid::Uuid::new_v4().to_string(), body: Some(text.into()), sent_at: now_ms(), ..Default::default() };
    let msg = message_encrypt(&Content::Data(dm).encode(), &remote, &local, &mut store.session_store, &mut store.identity_store, now, &mut rng)
        .await
        .unwrap();
    assert_eq!(msg.message_type(), CiphertextMessageType::PreKey);
    sql(&format!(
        "INSERT INTO envelopes (id, account_id, device_id, type, source_account, source_device, group_id, client_timestamp, server_timestamp, content) \
         VALUES ('{}', '{}', {}, 3, '{source}', {as_device}, NULL, {now}, {now}, FROM_BASE64('{}'))",
        uuid::Uuid::new_v4(),
        to.account_id,
        to.device_id,
        chat_core::crypto::b64(msg.serialize()),
        now = now_ms(),
    ));
}

/// The server's trust root for sealed sender certificates is pinned like its other keys: a
/// server that later presents another one (to vouch for certificates it never issued before)
/// does not get it accepted on the next connection.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_sealed_sender_trust_root_is_pinned() {
    if skip() {
        return;
    }
    let alice = Client::new("alice");
    register(&[&alice]).await;
    let mut served = None;
    for _ in 0..100 {
        served = alice.kv("sealed_trust_root").await;
        if served.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let served = served.expect("the trust root is pinned on first contact");

    // The pin is now another key than the one the server serves: from the client's point of
    // view, the server's key changed since it was pinned.
    let mut pinned = vec![0x05u8];
    pinned.extend_from_slice(&[0x42u8; 32]);
    assert_ne!(pinned, served);
    let p = pinned.clone();
    alice.core.run(move |i| async move { i.db.borrow().kv_set("sealed_trust_root", &p) }).await.unwrap();

    // Reconnecting runs the setup again: the pinned key stays.
    let alice = Client::start_again(alice.stop().await).await;
    for _ in 0..50 {
        assert_eq!(alice.kv("sealed_trust_root").await.as_deref(), Some(&pinned[..]), "the server replaced the pinned trust root");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// An identity pinned from an incoming first message was not checked against the key
/// transparency log. The server (with a modified client) sends Alice a first message in Bob's
/// name under a key of its own: Alice pins that key for Bob. Her reply must not be encrypted
/// for it: the key is checked against the log first, refused, and the conversation shows the
/// conflict.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_identity_pinned_from_an_incoming_message_is_checked_before_sending() {
    if skip() {
        return;
    }
    let (alice, bob, mallory) = (Client::new("alice"), Client::new("bob"), Client::new("mallory"));
    let acc = register(&[&alice, &bob, &mallory]).await;
    let (a, b) = (&acc[0], &acc[1]);
    // Bob and Alice are offline: whatever is sent to them waits on the server.
    let _bob = bob.stop().await;
    let alice = alice.stop().await;
    forge_first_message(&mallory, a, &b.account_id, b.device_id, &b.account_id, "hi alice, it's bob").await;

    let alice = Client::start_again(alice).await;
    let fake = alice.wait_message("hi alice, it's bob").await;
    assert_eq!(fake["sender_account_id"].as_str(), Some(b.account_id.as_str()), "the attack is in place");

    let conv = open_chat(&alice, b).await;
    let reply = send_text(&alice, &conv, "for bob only").await;
    alice.wait_status(&reply.id, "failed").await;
    alice.wait_kt_alert("key_mismatch").await;
    alice.wait_identity_conflict(&b.account_id).await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    let queued = sql(&format!("SELECT COUNT(*) FROM envelopes WHERE account_id = '{}'", b.account_id));
    assert_eq!(queued, "0", "nothing may be encrypted for the key the server slipped in");
}

/// The safety number of a contact we never exchanged a message with used the key the server's
/// directory served, unchecked; marking it verified was not tied to any key. The key must be
/// the one in the key transparency log, and "verified" needs a pinned key.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_safety_number_of_a_new_contact_is_the_key_in_the_log() {
    if skip() {
        return;
    }
    let (alice, bob, mallory) = (Client::new("alice"), Client::new("bob"), Client::new("mallory"));
    let acc = register(&[&alice, &bob, &mallory]).await;
    let (b, m) = (&acc[1], &acc[2]);
    open_chat(&alice, b).await;

    // The directory now serves Mallory's identity key for Bob.
    let original = sql(&format!("SELECT HEX(identity_key) FROM accounts WHERE id = '{}'", b.account_id));
    sql(&format!(
        "UPDATE accounts SET identity_key = (SELECT k FROM (SELECT identity_key AS k FROM accounts WHERE id = '{}') t) WHERE id = '{}'",
        m.account_id, b.account_id
    ));
    let id = b.account_id.clone();
    let number = alice.core.run(move |i| async move { i.safety_number(&id).await }).await;
    let id = b.account_id.clone();
    let verified = alice.core.run(move |i| async move { i.set_verified(&id, true) }).await;
    sql(&format!("UPDATE accounts SET identity_key = UNHEX('{original}') WHERE id = '{}'", b.account_id));

    assert!(number.is_err(), "a safety number from a key that is not in the log: {number:?}");
    alice.wait_kt_alert("key_mismatch").await;
    assert!(verified.is_err(), "nothing was pinned: there is no key to call verified");

    // With the real key back, the number is the one Bob sees, and it can be marked verified.
    let id = b.account_id.clone();
    let ours = alice.core.run(move |i| async move { i.safety_number(&id).await }).await.unwrap();
    let a_id = acc[0].account_id.clone();
    let theirs = bob.core.run(move |i| async move { i.safety_number(&a_id).await }).await.unwrap();
    assert_eq!(ours.digits, theirs.digits);
    let id = b.account_id.clone();
    let c = alice.core.run(move |i| async move { i.set_verified(&id, true) }).await.unwrap();
    assert!(c.verified);
}

/// The server delivers a message in Bob's name, under a key of its own, from Bob's account id
/// written in capitals. That is Bob's account: Bob's pinned identity applies (the key is
/// refused), it is not a new account to trust on first use.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_account_id_in_capitals_is_the_same_account() {
    if skip() {
        return;
    }
    let (alice, bob, mallory) = (Client::new("alice"), Client::new("bob"), Client::new("mallory"));
    let acc = register(&[&alice, &bob, &mallory]).await;
    let (a, b) = (&acc[0], &acc[1]);
    let conv = open_chat(&alice, b).await;
    send_text(&alice, &conv, "hi bob").await;
    bob.wait_message("hi bob").await;

    let alice = alice.stop().await;
    forge_first_message(&mallory, a, &b.account_id, b.device_id, &b.account_id.to_ascii_uppercase(), "bob here, in capitals").await;

    let alice = Client::start_again(alice).await;
    alice.wait_identity_conflict(&b.account_id).await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(!alice.saw_message("bob here, in capitals"), "a message under another key than Bob's pinned one");
}
