//! Regression tests for the findings of the November 2026 security assessment that concern
//! the core (F-04, F-05, F-08, F-09). Each one plays the attacker described in the finding:
//! a server that edits its own database, or a peer running a modified client.
//!
//! Needs the server and direct access to its MySQL database, as tests/evil_server.rs:
//! `CHATAPP_E2E_SERVER=http://127.0.0.1:8080 CHATAPP_E2E_MYSQL="-uchatapp -pchatapp chatapp_dev"
//!  cargo test -p chat-core --test evil_server_audit -- --nocapture`

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chat_core::content::{Content, DataMessage};
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
        // The local data is locked with the account password.
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

    fn saw_message(&self, text: &str) -> bool {
        self.events.0.lock().unwrap().iter().any(|(n, v)| n == "message" && v["message"]["text"].as_str() == Some(text))
    }

    async fn wait_online(&self) {
        self.wait_event("online", |n, v| n == "connection" && v.as_str() == Some("online")).await;
    }

    async fn message(&self, id: &str) -> Option<Message> {
        let id = id.to_string();
        self.core.run(move |i| async move { i.get_message(&id) }).await.ok()
    }

    /// The first column of a query on this client's own database (what the app stored).
    async fn local(&self, query: &'static str, arg: &str) -> Vec<String> {
        let arg = arg.to_string();
        self.core
            .run(move |i| async move {
                let db = i.db.borrow();
                let mut st = db.conn.prepare(query).unwrap();
                let rows = st.query_map([arg], |r| r.get::<_, String>(0)).unwrap();
                Ok(rows.map(|r| r.unwrap()).collect())
            })
            .await
            .unwrap()
    }

    async fn profile_key(&self) -> String {
        self.core.run(|i| async move { i.profile_key_b64() }).await.unwrap()
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

/// F-04. An admin removes a member, but the server keeps listing her (so that nobody notices
/// a smaller list) and goes on delivering the group's ciphertext to her. The remaining members
/// must start a new sender key chain because of the admin's end-to-end attested list, not
/// because of what the server lists: the removed member holds the old chain.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn f04_member_removed_end_to_end_but_kept_by_the_server_gets_no_usable_sender_key() {
    if skip() {
        return;
    }
    let (alice, bob, carol) = (Client::new("alice"), Client::new("bob"), Client::new("carol"));
    let acc = register(&[&alice, &bob, &carol]).await;
    let req: CreateGroupRequest =
        serde_json::from_value(json!({ "name": "trio", "member_account_ids": [acc[1].account_id, acc[2].account_id] })).unwrap();
    let g = alice.core.run(move |i| async move { i.create_group(req).await }).await.unwrap();
    let conv = g.id.clone();
    let gid = alice.core.run(move |i| async move { i.group_info(&conv) }).await.unwrap().group_id;
    assert_eq!(sql(&format!("SELECT COUNT(*) FROM group_members WHERE group_id = '{gid}'")), "3", "a v1 group with three members");

    send_text(&alice, &g.id, "hello all").await;
    let bob_conv = bob.wait_message("hello all").await["conversation_id"].as_str().unwrap().to_string();
    carol.wait_message("hello all").await;
    // Bob speaks: Carol now holds his sender key chain.
    send_text(&bob, &bob_conv, "bob before").await;
    alice.wait_message("bob before").await;
    carol.wait_message("bob before").await;
    const DIST: &str = "SELECT distribution_id FROM groups WHERE group_id = ?1";
    let before = bob.local(DIST, &gid).await;
    assert_eq!(before.len(), 1, "bob's distribution id");

    // Bob is offline while Alice removes Carol; the server then puts Carol back on its list.
    let bob = bob.stop().await;
    let (conv, cid) = (g.id.clone(), acc[2].account_id.clone());
    alice.core.run(move |i| async move { i.remove_group_member(&conv, &cid).await }).await.unwrap();
    sql(&format!(
        "INSERT INTO group_members (group_id, account_id, role, joined_at, added_by) VALUES ('{gid}', '{carol}', 'member', {now}, '{alice}');
         UPDATE chat_groups SET revision = revision + 1 WHERE id = '{gid}';",
        carol = acc[2].account_id,
        alice = acc[0].account_id,
        now = now_ms()
    ));
    assert_eq!(sql(&format!("SELECT COUNT(*) FROM group_members WHERE group_id = '{gid}'")), "3", "the server still lists Carol");

    // Bob comes back: the server's list never got smaller for him, the admin's list did.
    let bob = Client::start_again(bob).await;
    let mut info = None;
    for _ in 0..300 {
        let cv = bob_conv.clone();
        let i = bob.core.run(move |i| async move { i.group_info(&cv) }).await.unwrap();
        if i.members.iter().any(|m| m.account_id == acc[2].account_id && !m.verified) {
            info = Some(i);
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let info = info.expect("bob learns from the admin that Carol is no longer a member");
    assert_eq!(info.members.len(), 3, "the attack is in place: bob's copy of the server list still has Carol");

    let after = bob.local(DIST, &gid).await;
    assert_ne!(before, after, "bob must start a new sender key chain: Carol holds the old one");

    // The group goes on for the remaining members; Carol (served the ciphertext) reads nothing.
    send_text(&bob, &bob_conv, "bob after").await;
    alice.wait_message("bob after").await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(!carol.saw_message("bob after"), "the removed member must not read later messages");
}

/// The vault records of an account as the server stores them: record id -> hex(data).
fn vault_snapshot(account: &str) -> BTreeMap<String, String> {
    sql(&format!("SELECT record_id, HEX(data) FROM vault_records WHERE account_id = '{account}' AND data IS NOT NULL"))
        .lines()
        .filter_map(|l| l.split_once('\t'))
        .map(|(id, data)| (id.to_string(), data.to_string()))
        .collect()
}

/// Waits until the vault stops changing (the pending uploads went out), then returns it.
async fn settled_vault(account: &str) -> BTreeMap<String, String> {
    let mut last = vault_snapshot(account);
    let mut same = 0;
    for _ in 0..120 {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let now = vault_snapshot(account);
        same = if now == last { same + 1 } else { 0 };
        last = now;
        if same >= 6 {
            return last;
        }
    }
    panic!("the vault of {account} never settled");
}

/// F-05. After blocking someone the profile key is rotated, so the blocked account loses our
/// profile, presence and sealed delivery. The server keeps the vault: it puts back the records
/// it stored before the rotation (under new sequence numbers, so they look fresh). The client
/// must not go back to the key the blocked account knows.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn f05_vault_rolled_back_by_the_server_does_not_restore_a_retired_profile_key() {
    if skip() {
        return;
    }
    let (alice, bob, carol) = (Client::new("alice"), Client::new("bob"), Client::new("carol"));
    let acc = register(&[&alice, &bob, &carol]).await;
    let me = acc[0].account_id.clone();
    for (peer, text) in [(&acc[1], "hi bob"), (&acc[2], "hi carol")] {
        let conv = open_chat(&alice, peer).await;
        send_text(&alice, &conv, text).await;
    }
    bob.wait_message("hi bob").await;
    carol.wait_message("hi carol").await;
    let k0 = alice.profile_key().await;

    // First rotation (Bob is blocked): the vault now holds the settings record with K1.
    let id = acc[1].account_id.clone();
    alice.core.run(move |i| async move { i.set_blocked(&id, true) }).await.unwrap();
    let mut k1 = k0.clone();
    for _ in 0..300 {
        k1 = alice.profile_key().await;
        if k1 != k0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_ne!(k1, k0, "blocking Bob rotates the profile key");
    let old_vault = settled_vault(&me).await;

    // Second rotation (Carol is blocked): K1 is retired, Carol knows it.
    let id = acc[2].account_id.clone();
    alice.core.run(move |i| async move { i.set_blocked(&id, true) }).await.unwrap();
    let mut k2 = k1.clone();
    for _ in 0..300 {
        k2 = alice.profile_key().await;
        if k2 != k1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_ne!(k2, k1, "blocking Carol rotates the profile key again");
    let new_vault = settled_vault(&me).await;
    let rolled_back: Vec<&String> = old_vault.iter().filter(|(id, data)| new_vault.get(*id) != Some(*data)).map(|(id, _)| id).collect();
    assert!(!rolled_back.is_empty(), "the rotation changed records the server can roll back");

    // The server rolls the account's vault back to what it stored before the second rotation.
    let mut seq: i64 = sql("SELECT value FROM vault_counter WHERE id = 1").parse().unwrap();
    let mut script = String::new();
    for (id, data) in &old_vault {
        seq += 1;
        script.push_str(&format!(
            "UPDATE vault_records SET data = X'{data}', size = {size}, seq = {seq} WHERE account_id = '{me}' AND record_id = '{id}';",
            size = data.len() / 2
        ));
    }
    script.push_str(&format!("UPDATE vault_counter SET value = {seq} WHERE id = 1;"));
    sql(&script);

    // Alice's app starts again and pulls the vault, old records included.
    let alice = Client::start_again(alice.stop().await).await;
    let mut pulled = 0;
    for _ in 0..300 {
        let v = alice.local("SELECT CAST(value AS TEXT) FROM kv WHERE key = ?1", "vault_seq").await;
        pulled = v.first().and_then(|s| s.parse::<i64>().ok()).unwrap_or(0);
        if pulled >= seq {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(pulled >= seq, "alice pulled the rolled-back records (at {pulled}, they end at {seq})");
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(alice.profile_key().await, k2, "a vault replay must not bring back the key Carol knows");
}

/// The server puts back what it stored earlier for these records, under new sequence numbers
/// (so they look like fresh changes). Returns the last sequence number used.
fn roll_back(account: &str, old: &BTreeMap<String, String>, ids: &[String]) -> i64 {
    let mut seq: i64 = sql("SELECT value FROM vault_counter WHERE id = 1").parse().unwrap();
    let mut script = String::new();
    for id in ids {
        let data = &old[id];
        seq += 1;
        script.push_str(&format!(
            "UPDATE vault_records SET data = X'{data}', size = {size}, seq = {seq} WHERE account_id = '{account}' AND record_id = '{id}';",
            size = data.len() / 2
        ));
    }
    script.push_str(&format!("UPDATE vault_counter SET value = {seq} WHERE id = 1;"));
    sql(&script);
    seq
}

/// Alice with two chats; then a one-hour timer for the chat with Bob, and Carol blocked.
/// Returns (alice's vault before those two changes, the ids of the records they changed).
async fn timer_and_block(alice: &Client, acc: &[AccountInfo]) -> (BTreeMap<String, String>, Vec<String>) {
    let me = acc[0].account_id.clone();
    let mut bob_conv = String::new();
    for (peer, text) in [(&acc[2], "hi carol"), (&acc[1], "hi bob")] {
        bob_conv = open_chat(alice, peer).await;
        send_text(alice, &bob_conv, text).await;
    }
    let old = settled_vault(&me).await;
    let patch: ConversationSettingsPatch = serde_json::from_value(json!({ "conversation_id": bob_conv, "disappearing_seconds": 3600 })).unwrap();
    alice.core.run(move |i| async move { i.set_conversation_settings(patch).await }).await.unwrap();
    let id = acc[2].account_id.clone();
    alice.core.run(move |i| async move { i.set_blocked(&id, true) }).await.unwrap();
    let new = settled_vault(&me).await;
    let changed: Vec<String> = old.iter().filter(|(id, data)| new.get(*id) != Some(*data)).map(|(id, _)| id.clone()).collect();
    assert!(changed.len() >= 2, "the timer and the block changed records the server can roll back");
    (old, changed)
}

const TIMER: &str = "SELECT CAST(disappearing_seconds AS TEXT) FROM conversations WHERE peer_account_id = ?1";

async fn blocked(c: &Client, account: &str) -> bool {
    let id = account.to_string();
    c.core.run(move |i| async move { Ok(i.is_blocked(&id)) }).await.unwrap()
}

/// F-05, every record: the server rolls back the records of a chat's timer and of a blocked
/// contact. The device that made those changes refuses the older copies, keeps its state and
/// uploads it again, so the vault itself is repaired.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn f05_rolled_back_records_are_refused_and_uploaded_again() {
    if skip() {
        return;
    }
    let (alice, bob, carol) = (Client::new("alice"), Client::new("bob"), Client::new("carol"));
    let acc = register(&[&alice, &bob, &carol]).await;
    let me = acc[0].account_id.clone();
    let (old, changed) = timer_and_block(&alice, &acc).await;
    let seq = roll_back(&me, &old, &changed);

    let alice = Client::start_again(alice.stop().await).await;
    let mut pulled = 0;
    for _ in 0..300 {
        let v = alice.local("SELECT CAST(value AS TEXT) FROM kv WHERE key = ?1", "vault_seq").await;
        pulled = v.first().and_then(|s| s.parse::<i64>().ok()).unwrap_or(0);
        if pulled >= seq {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(pulled >= seq, "alice pulled the rolled-back records");
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(alice.local(TIMER, &acc[1].account_id).await, ["3600"], "the chat's timer was rolled back");
    assert!(blocked(&alice, &acc[2].account_id).await, "the block was rolled back");

    // The server's copies are replaced by Alice's again.
    let mut stale = changed.len();
    for _ in 0..300 {
        let now = vault_snapshot(&me);
        stale = changed.iter().filter(|id| now.get(*id) == old.get(*id)).count();
        if stale == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(stale, 0, "the rolled-back records were not uploaded again");
}

/// F-05, a new device: the server rolls the same records back and then refuses to store
/// anything newer for them, while Alice links a second device. The new device has no state of
/// its own, but the linking device vouched (end to end) for the versions the vault must have:
/// it notices that the vault falls short, and takes the real state once the server lets it
/// through.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn f05_a_newly_linked_device_does_not_settle_for_a_rolled_back_vault() {
    if skip() {
        return;
    }
    let (alice, bob, carol) = (Client::new("alice"), Client::new("bob"), Client::new("carol"));
    let acc = register(&[&alice, &bob, &carol]).await;
    let me = acc[0].account_id.clone();
    let (old, changed) = timer_and_block(&alice, &acc).await;
    roll_back(&me, &old, &changed);
    sql("DROP TRIGGER IF EXISTS freeze_vault");
    sql(&format!(
        "CREATE TRIGGER freeze_vault BEFORE UPDATE ON vault_records FOR EACH ROW
         SET NEW.data = IF(OLD.account_id = '{me}' AND FIND_IN_SET(OLD.record_id, '{ids}') > 0, OLD.data, NEW.data)",
        ids = changed.join(",")
    ));

    let alice2 = Client::new("alice2");
    let u = server();
    let link = alice2.core.run(move |i| async move { i.link_create(&u, "second").await }).await.unwrap();
    let waiter = {
        let core = alice2.core.clone();
        tokio::spawn(async move { core.run(|i| async move { i.link_wait().await }).await })
    };
    alice.core.run(move |i| async move { i.link_approve(Some(&link)).await }).await.unwrap();
    waiter.await.unwrap().unwrap();
    alice2.wait_online().await;
    let seen = alice2.wait_event("the vault found incomplete", |n, v| n == "vault_incomplete" && v["missing"].as_i64().is_some_and(|m| m > 0)).await;
    assert!(seen["missing"].as_i64().unwrap() >= changed.len() as i64);
    assert!(!blocked(&alice2, &acc[2].account_id).await, "nothing of the rolled-back records is applied meanwhile");

    // The server gives in: the new device keeps asking, and Alice's first device uploads the
    // records again (nobody has to do anything).
    sql("DROP TRIGGER freeze_vault");
    let mut done = false;
    for _ in 0..40 {
        tokio::time::sleep(Duration::from_secs(3)).await;
        if alice2.local(TIMER, &acc[1].account_id).await == ["3600"] && blocked(&alice2, &acc[2].account_id).await {
            done = true;
            break;
        }
    }
    assert!(done, "the new device never got the real timer and block");
}

/// F-05, no other device: signing in with the password has nothing but the server's vault to
/// go by, so a profile key found there may be one the account had rotated away from. The key
/// is rotated once the restore is done, on every device of the account.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn f05_signing_in_with_the_password_rotates_the_profile_key() {
    if skip() {
        return;
    }
    let (alice, bob) = (Client::new("alice"), Client::new("bob"));
    let acc = register(&[&alice, &bob]).await;
    let conv = open_chat(&alice, &acc[1]).await;
    send_text(&alice, &conv, "hi bob").await;
    bob.wait_message("hi bob").await;
    settled_vault(&acc[0].account_id).await;
    let k0 = alice.profile_key().await;

    let alice2 = Client::new("alice2");
    let (u, number) = (server(), acc[0].number.clone());
    alice2.core.run(move |i| async move { i.login(&u, &number, PASSWORD, "second", None).await }).await.unwrap();
    alice2.wait_online().await;
    alice2.wait_event("history restored", |n, v| n == "history_sync" && v["done"] == true).await;
    let mut k = k0.clone();
    for _ in 0..300 {
        k = alice2.profile_key().await;
        if k != k0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_ne!(k, k0, "the restored device keeps the profile key it found in the vault");
    let mut same = false;
    for _ in 0..300 {
        if alice.profile_key().await == k {
            same = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(same, "the first device did not get the new profile key");
}

/// F-08. A stranger sends a pairwise message tagged with a group id that is a path, not an id.
/// The id used to be put into the request path `/v1/groups/{id}` as received. Here it points
/// (through "./") at a real group of the victim, which makes the effect visible: the victim's
/// app stored a second group under the forged id.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn f08_group_id_from_a_message_is_never_used_as_a_path() {
    if skip() {
        return;
    }
    let (alice, mallory) = (Client::new("alice"), Client::new("mallory"));
    let acc = register(&[&alice, &mallory]).await;
    let req: CreateGroupRequest = serde_json::from_value(json!({ "name": "bait", "member_account_ids": [acc[0].account_id] })).unwrap();
    let g = mallory.core.run(move |i| async move { i.create_group(req).await }).await.unwrap();
    let conv = g.id.clone();
    let gid = mallory.core.run(move |i| async move { i.group_info(&conv) }).await.unwrap().group_id;
    const ROW: &str = "SELECT group_id FROM groups WHERE group_id = ?1";
    for _ in 0..300 {
        if !alice.local(ROW, &gid).await.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(alice.local(ROW, &gid).await.len(), 1, "alice knows the real group");

    for forged in [format!("./{gid}"), format!("../groups/{gid}"), format!("{gid}?x=1"), format!("{gid}/")] {
        let dm = DataMessage {
            id: uuid::Uuid::new_v4().to_string(),
            group_id: Some(forged),
            body: Some("forged".into()),
            sent_at: now_ms(),
            ..Default::default()
        };
        let to = acc[0].account_id.clone();
        mallory.core.run(move |i| async move { i.send_to_account(&to, &Content::Data(dm), false).await }).await.unwrap();
    }
    // A normal message afterwards: once it arrives, the forged ones were handled.
    let conv = open_chat(&mallory, &acc[0]).await;
    send_text(&mallory, &conv, "marker").await;
    alice.wait_message("marker").await;

    let ids = alice.local("SELECT group_id FROM groups WHERE ?1 = ''", "").await;
    assert_eq!(ids, vec![gid.clone()], "only the real group: a forged id must not reach the server as a path");
    assert!(!alice.saw_message("forged"));
}

/// F-09. The other side of a chat sets a 1 s disappearing timer. It applies to new messages
/// (the legitimate use), but must not make the messages we already have disappear. Messages
/// already in the chat keep the deadline they were sent with, and clearing one's own history
/// stays a local choice.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn f09_a_peer_cannot_make_our_history_disappear_with_a_timer() {
    if skip() {
        return;
    }
    let (alice, bob) = (Client::new("alice"), Client::new("bob"));
    let acc = register(&[&alice, &bob]).await;
    let conv = open_chat(&alice, &acc[1]).await;
    let mine = send_text(&alice, &conv, "alice's old message").await;
    let bob_conv = bob.wait_message("alice's old message").await["conversation_id"].as_str().unwrap().to_string();
    let bc = bob_conv.clone();
    bob.core.run(move |i| async move { i.accept_conversation(&bc).await }).await.unwrap();
    let theirs = send_text(&bob, &bob_conv, "bob's old message").await;
    alice.wait_message("bob's old message").await;
    for c in [(&alice, conv.clone()), (&bob, bob_conv.clone())] {
        let cv = c.1.clone();
        c.0.core.run(move |i| async move { i.mark_read(&cv) }).await.unwrap();
    }
    // A message sent under a one-hour timer: it keeps that deadline whatever comes later.
    let patch: ConversationSettingsPatch = serde_json::from_value(json!({ "conversation_id": conv, "disappearing_seconds": 3600 })).unwrap();
    alice.core.run(move |i| async move { i.set_conversation_settings(patch).await }).await.unwrap();
    let hourly = send_text(&alice, &conv, "alice's one-hour message").await;
    let deadline = hourly.expires_at.expect("sent under the one-hour timer");
    bob.wait_message("alice's one-hour message").await;
    tokio::time::sleep(Duration::from_millis(2500)).await;

    // Bob (the peer) turns on a 1 s timer.
    let patch: ConversationSettingsPatch = serde_json::from_value(json!({ "conversation_id": bob_conv, "disappearing_seconds": 1 })).unwrap();
    bob.core.run(move |i| async move { i.set_conversation_settings(patch).await }).await.unwrap();

    // Legitimate: the timer reached Alice and new messages follow it, in both directions.
    let fresh = send_text(&bob, &bob_conv, "bob's timed message").await;
    assert!(fresh.expires_at.is_some());
    alice.wait_message("bob's timed message").await;
    let reply = send_text(&alice, &conv, "alice's timed message").await;
    assert!(reply.expires_at.is_some(), "alice took the timer: her new messages are timed too");
    let cv = conv.clone();
    alice.core.run(move |i| async move { i.mark_read(&cv) }).await.unwrap();
    let (fid, rid) = (fresh.id.clone(), reply.id.clone());
    alice.wait_event("timed message expires for alice", move |n, v| n == "message_deleted" && v["message_id"] == fid.as_str()).await;
    alice.wait_event("alice's timed message expires", move |n, v| n == "message_deleted" && v["message_id"] == rid.as_str()).await;

    // The attack: Alice's history from before the timer is still there, seconds later.
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(alice.message(&mine.id).await.is_some(), "a peer's timer must not delete our own earlier messages");
    assert!(alice.message(&theirs.id).await.is_some(), "a peer's timer must not delete the earlier messages we received");
    assert_eq!(alice.message(&hourly.id).await.and_then(|m| m.expires_at), Some(deadline), "an existing message keeps the deadline it was sent with");
    // Nor does the one who set the timer lose what was there before.
    assert!(bob.message(&mine.id).await.is_some() && bob.message(&theirs.id).await.is_some(), "the timer is for new messages, also for the one who set it");

    // A modified client sends the bare timer change again and again: still nothing is lost.
    for _ in 0..3 {
        let dm = DataMessage { id: uuid::Uuid::new_v4().to_string(), sent_at: now_ms(), expire_timer: 1, timer_update: true, ..Default::default() };
        let to = acc[0].account_id.clone();
        bob.core.run(move |i| async move { i.send_to_account(&to, &Content::Data(dm), false).await }).await.unwrap();
    }
    send_text(&bob, &bob_conv, "after the forged timers").await;
    alice.wait_message("after the forged timers").await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(alice.message(&mine.id).await.is_some() && alice.message(&theirs.id).await.is_some(), "forged timer updates must not delete history");

    // Clearing a chat is each user's own choice: Alice empties hers, Bob's copy is untouched.
    let cv = conv.clone();
    alice.core.run(move |i| async move { i.clear_conversation(&cv) }).await.unwrap();
    assert!(alice.message(&mine.id).await.is_none() && alice.message(&theirs.id).await.is_none(), "alice cleared her own history");
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(bob.message(&mine.id).await.is_some() && bob.message(&theirs.id).await.is_some(), "clearing a chat is local");
}

/// F-09 in a group: the timer is a group rule set by admins and stored by the server. A new
/// timer must not make the members' existing group history disappear, whether an admin sets
/// it or the server edits the group.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn f09_a_group_timer_does_not_expire_the_members_history() {
    if skip() {
        return;
    }
    let (alice, bob) = (Client::new("alice"), Client::new("bob"));
    let acc = register(&[&alice, &bob]).await;
    let req: CreateGroupRequest = serde_json::from_value(json!({ "name": "duo", "member_account_ids": [acc[1].account_id] })).unwrap();
    let g = alice.core.run(move |i| async move { i.create_group(req).await }).await.unwrap();
    let conv = g.id.clone();
    let gid = alice.core.run(move |i| async move { i.group_info(&conv) }).await.unwrap().group_id;
    let first = send_text(&alice, &g.id, "group message one").await;
    let got = bob.wait_message("group message one").await;
    let bob_conv = got["conversation_id"].as_str().unwrap().to_string();
    let second = send_text(&bob, &bob_conv, "group message two").await;
    alice.wait_message("group message two").await;
    let cv = bob_conv.clone();
    bob.core.run(move |i| async move { i.mark_read(&cv) }).await.unwrap();
    tokio::time::sleep(Duration::from_millis(2500)).await;

    // The admin turns on a 1 s timer for the group.
    let patch: ConversationSettingsPatch = serde_json::from_value(json!({ "conversation_id": g.id, "disappearing_seconds": 1 })).unwrap();
    alice.core.run(move |i| async move { i.set_conversation_settings(patch).await }).await.unwrap();
    bob.wait_event("group timer", |n, v| n == "conversation_updated" && v["disappearing_seconds"] == 1).await;
    // New messages follow it.
    let timed = send_text(&alice, &g.id, "timed group message").await;
    assert!(timed.expires_at.is_some());
    bob.wait_message("timed group message").await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    for (who, c) in [("alice", &alice), ("bob", &bob)] {
        assert!(c.message(&first.id).await.is_some() && c.message(&second.id).await.is_some(), "{who}: the group timer deleted existing messages");
    }

    // The server edits the group's timer by itself (off, then 1 s again under a new revision).
    sql(&format!("UPDATE chat_groups SET disappearing_seconds = 0, revision = revision + 1 WHERE id = '{gid}'"));
    send_text(&alice, &g.id, "between").await;
    bob.wait_message("between").await;
    sql(&format!("UPDATE chat_groups SET disappearing_seconds = 1, revision = revision + 1 WHERE id = '{gid}'"));
    let bob = Client::start_again(bob.stop().await).await;
    let cv = bob_conv.clone();
    for _ in 0..300 {
        let c = cv.clone();
        if bob.core.run(move |i| async move { i.group_info(&c) }).await.is_ok_and(|g| g.settings.disappearing_seconds == 1) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(bob.message(&first.id).await.is_some() && bob.message(&second.id).await.is_some(), "a server-set group timer deleted existing messages");
}
