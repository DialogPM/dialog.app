//! End-to-end tests of private groups v2 (zkgroup, docs/ZKGROUP-DESIGN.md stages 2-3).
//!
//! Run against server-rs (rate limits disabled), e.g.:
//! `CHATAPP_E2E_SERVER=http://127.0.0.1:8095 cargo test -p chat-core --test e2e_groups_v2 -- --test-threads=1`
//! With `CHATAPP_E2E_SQL` (a shell command running SQL from stdin against the server's
//! database, see tests/e2e.rs) the server's tables are also checked for plaintext account ids.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use chat_core::crypto::b64;
use chat_core::model::*;
use chat_core::{Core, CoreConfig, EventSink};
use serde_json::{json, Value};
use zkgroup::groups::{GroupMasterKey, GroupSecretParams};

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
            if let Some((_, v)) = self.events.0.lock().unwrap().iter().find(|(n, v)| pred(n, v)) {
                return v.clone();
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("{}: timed out waiting for {what}", self.name);
    }

    async fn wait_message(&self, text: &str) -> Value {
        let t = text.to_string();
        self.wait_event(&format!("message '{text}'"), move |n, v| n == "message" && v["message"]["text"].as_str() == Some(t.as_str()))
            .await["message"]
            .clone()
    }

    fn saw_message(&self, text: &str) -> bool {
        self.events.0.lock().unwrap().iter().any(|(n, v)| n == "message" && v["message"]["text"].as_str() == Some(text))
    }

    async fn wait_online(&self) {
        self.wait_event("online", |n, v| n == "connection" && v.as_str() == Some("online")).await;
    }

    async fn group_info(&self, conv: &str) -> GroupInfo {
        let id = conv.to_string();
        self.core.run(move |i| async move { i.group_info(&id) }).await.unwrap()
    }

    /// Polls the group info until `pred` holds.
    async fn wait_group(&self, conv: &str, what: &str, pred: impl Fn(&GroupInfo) -> bool) -> GroupInfo {
        for _ in 0..150 {
            let id = conv.to_string();
            if let Ok(g) = self.core.run(move |i| async move { i.group_info(&id) }).await {
                if pred(&g) {
                    return g;
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("{}: timed out waiting for group: {what}", self.name);
    }
}

fn server() -> String {
    std::env::var("CHATAPP_E2E_SERVER").unwrap_or_default()
}

async fn v2_server() -> Option<String> {
    let url = server();
    if url.is_empty() {
        return None;
    }
    let info: Value = reqwest::get(format!("{url}/v1/info")).await.unwrap().json().await.unwrap();
    if info["groups_v2"].as_bool() != Some(true) {
        eprintln!("server without groups v2, skipping");
        return None;
    }
    Some(url)
}

async fn register_all(url: &str, clients: &[&Client]) -> Vec<AccountInfo> {
    let mut out = Vec::new();
    for c in clients {
        let u = url.to_string();
        let r = c.core.run(move |i| async move { i.register(&u, "correct horse battery", "test device").await }).await.unwrap();
        out.push(r.account);
        c.wait_online().await;
    }
    out
}

async fn send_text(c: &Client, conv: &str, text: &str) {
    let req: SendMessageRequest = serde_json::from_value(json!({ "conversation_id": conv, "text": text })).unwrap();
    c.core.run(move |i| async move { i.send_message(req).await }).await.unwrap();
}

async fn add_contact(c: &Client, number: &str) {
    let n = number.to_string();
    c.core.run(move |i| async move { i.add_contact_by_number(&n, None).await }).await.unwrap();
}

fn run_sql(sql_cmd: &str, sql: &str) -> String {
    use std::io::Write;
    let mut child = std::process::Command::new("sh")
        .args(["-c", sql_cmd])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("run CHATAPP_E2E_SQL");
    child.stdin.take().unwrap().write_all(sql.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "SQL failed");
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// A v2 group from creation to removal: the members see the same group, messages flow only to
/// current members, admin operations go through the anonymous v2 API, and the server's tables
/// never hold an account id or the group's name.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn v2_group_lifecycle() {
    let Some(url) = v2_server().await else { return };
    let (ana, bob, carl, dan) = (Client::new("ana"), Client::new("bob"), Client::new("carl"), Client::new("dan"));
    let acc = register_all(&url, &[&ana, &bob, &carl, &dan]).await;
    for a in &acc[1..] {
        add_contact(&ana, &a.number).await;
    }
    for c in [&bob, &carl, &dan] {
        add_contact(c, &acc[0].number).await;
    }
    let (a_id, b_id, c_id, d_id) = (acc[0].account_id.clone(), acc[1].account_id.clone(), acc[2].account_id.clone(), acc[3].account_id.clone());

    let req: CreateGroupRequest = serde_json::from_value(json!({
        "name": "Secret Squirrels", "description": "v2 group", "member_account_ids": [b_id, c_id],
        "settings": { "disappearing_seconds": 3600 },
    }))
    .unwrap();
    let g = ana.core.run(move |i| async move { i.create_group_v2(req).await }).await.unwrap();
    let conv = g.id.clone();
    let info = ana.group_info(&conv).await;
    let gid = info.group_id.clone();
    let gid2 = gid.clone();
    assert!(ana.core.run(move |i| async move { Ok(i.is_group_v2(&gid2)) }).await.unwrap());
    assert_eq!(info.members.len(), 3);
    assert!(info.members.iter().all(|m| m.verified));
    assert_eq!(info.my_role.as_deref(), Some("admin"));
    assert_eq!(info.settings.disappearing_seconds, 3600);
    // A full sync (v1 list + local v2 groups) keeps it.
    let gid2 = gid.clone();
    let still = ana.core.run(move |i| async move { i.sync_all_groups().await; Ok(i.is_group_v2(&gid2) && !i.group_member_ids(&gid2)?.is_empty()) }).await.unwrap();
    assert!(still);

    // Members get the master key end to end and decrypt the state from the server.
    let bob_conv = bob
        .wait_event("bob's v2 group", |n, v| n == "conversation_updated" && v["title"].as_str() == Some("Secret Squirrels"))
        .await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let carl_conv = carl
        .wait_event("carl's v2 group", |n, v| n == "conversation_updated" && v["title"].as_str() == Some("Secret Squirrels"))
        .await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let bi = bob.wait_group(&bob_conv, "3 verified members", |g| g.members.len() == 3 && g.members.iter().all(|m| m.verified)).await;
    assert_eq!(bi.group_id, gid);
    assert_eq!(bi.description, "v2 group");
    assert_eq!(bi.settings.disappearing_seconds, 3600);

    // Sender keys over the v2 send route.
    send_text(&ana, &conv, "hello v2").await;
    bob.wait_message("hello v2").await;
    carl.wait_message("hello v2").await;
    send_text(&bob, &bob_conv, "bob in v2").await;
    ana.wait_message("bob in v2").await;
    carl.wait_message("bob in v2").await;

    // Adding a member.
    let (c2, d) = (conv.clone(), d_id.clone());
    let gi = ana.core.run(move |i| async move { i.add_group_members(&c2, vec![d]).await }).await.unwrap();
    assert_eq!(gi.members.len(), 4);
    let dan_conv = dan
        .wait_event("dan's v2 group", |n, v| n == "conversation_updated" && v["title"].as_str() == Some("Secret Squirrels"))
        .await["id"]
        .as_str()
        .unwrap()
        .to_string();
    dan.wait_group(&dan_conv, "4 members", |g| g.members.len() == 4).await;
    send_text(&ana, &conv, "welcome dan").await;
    dan.wait_message("welcome dan").await;

    // Removing a member: it is out on the server and gets nothing more.
    let (c2, c) = (conv.clone(), c_id.clone());
    let gi = ana.core.run(move |i| async move { i.remove_group_member(&c2, &c).await }).await.unwrap();
    assert_eq!(gi.members.len(), 3);
    carl.wait_group(&carl_conv, "carl removed", |g| g.my_role.is_none()).await;
    bob.wait_group(&bob_conv, "carl gone", |g| g.members.len() == 3 && g.members.iter().all(|m| m.account_id != c_id)).await;
    send_text(&ana, &conv, "after carl").await;
    bob.wait_message("after carl").await;
    dan.wait_message("after carl").await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(!carl.saw_message("after carl"), "a removed member must not get group messages");
    // Carl still holds the master key, but the server refuses its presentations.
    let g2 = gid.clone();
    carl.core.run(move |i| async move { i.sync_group(&g2, None).await }).await.unwrap();
    assert!(carl.group_info(&carl_conv).await.my_role.is_none());

    // A non-admin cannot manage members (refused locally and by the server).
    let (bc, a) = (bob_conv.clone(), a_id.clone());
    assert!(bob.core.run(move |i| async move { i.remove_group_member(&bc, &a).await }).await.is_err());

    // Promotion, then the new admin renames the group.
    let (c2, b) = (conv.clone(), b_id.clone());
    ana.core.run(move |i| async move { i.set_group_admin(&c2, &b, true).await }).await.unwrap();
    bob.wait_group(&bob_conv, "bob admin", |g| g.my_role.as_deref() == Some("admin")).await;
    let upd: UpdateGroupRequest = serde_json::from_value(json!({ "conversation_id": bob_conv, "name": "Renamed Squirrels" })).unwrap();
    bob.core.run(move |i| async move { i.update_group(upd).await }).await.unwrap();
    ana.wait_group(&conv, "renamed", |g| g.name == "Renamed Squirrels").await;

    // Bob leaves.
    let bc = bob_conv.clone();
    bob.core.run(move |i| async move { i.leave_group(&bc).await }).await.unwrap();
    ana.wait_group(&conv, "bob left", |g| g.members.iter().all(|m| m.account_id != b_id)).await;

    // The change log is signed by the server (verified against the pinned parameters).
    let g2 = gid.clone();
    let changes = ana.core.run(move |i| async move { i.group_v2_changes(&g2, 0).await }).await.unwrap();
    assert!(changes.len() >= 5, "changes: {changes:?}");
    assert_eq!(changes[0]["revision"], 1);

    // The server's tables: no account id, no name, only ciphertexts.
    let sql_cmd = std::env::var("CHATAPP_E2E_SQL").unwrap_or_default();
    if sql_cmd.is_empty() {
        eprintln!("CHATAPP_E2E_SQL not set, skipping the database check");
        return;
    }
    let g2 = gid.clone();
    let server_id = ana.core.run(move |i| async move { i.group_v2_server_id(&g2) }).await.unwrap();
    let dump = run_sql(
        &sql_cmd,
        &format!(
            "SELECT HEX(id), HEX(public_params), revision, HEX(attributes), HEX(settings) FROM groups_v2 WHERE id = UNHEX('{server_id}');
             SELECT HEX(uid_ciphertext), role, joined_revision FROM group_members_v2 WHERE group_id = UNHEX('{server_id}');
             SELECT HEX(uid_ciphertext), HEX(added_by) FROM group_pending_v2 WHERE group_id = UNHEX('{server_id}');
             SELECT revision, `change`, HEX(`change`), HEX(signature) FROM group_changes_v2 WHERE group_id = UNHEX('{server_id}');"
        ),
    );
    assert!(dump.contains(&server_id), "the group must be in the v2 tables: {dump}");
    let members = run_sql(&sql_cmd, &format!("SELECT COUNT(*) FROM group_members_v2 WHERE group_id = UNHEX('{server_id}');"));
    assert!(members.lines().any(|l| l.trim() == "2"), "ana and dan remain: {members}");
    let upper = dump.to_uppercase();
    for a in &acc {
        let hex = a.account_id.replace('-', "").to_uppercase();
        assert!(!upper.contains(&a.account_id.to_uppercase()), "account id {} in the v2 tables", a.account_id);
        assert!(!upper.contains(&hex), "raw account id {} in the v2 tables", a.account_id);
    }
    let name_hex: String = "Squirrels".bytes().map(|b| format!("{b:02X}")).collect();
    assert!(!upper.contains(&name_hex) && !dump.contains("Squirrels"), "the group name in the v2 tables");
    // Nor in the v1 tables.
    let v1 = run_sql(&sql_cmd, &format!("SELECT COUNT(*) FROM chat_groups WHERE id = '{gid}';"));
    assert!(v1.lines().any(|l| l.trim() == "0"), "a v2 group must not exist in chat_groups: {v1}");
}

fn group_auth(params: &GroupSecretParams, presentation: &[u8]) -> String {
    format!("{}:{}", b64(zkgroup::serialize(&params.get_public_params())), b64(presentation))
}

fn uid(params: &GroupSecretParams, account: &str) -> String {
    let aci = libsignal_core::Aci::from(uuid::Uuid::parse_str(account).unwrap());
    b64(zkgroup::serialize(&params.encrypt_service_id(aci.into())))
}

async fn presentation(c: &Client, params: GroupSecretParams) -> Vec<u8> {
    c.core.run(move |i| async move { i.zk_auth_presentation(&params).await }).await.unwrap()
}

async fn call(url: &str, method: reqwest::Method, path: &str, auth: &str, body: Option<Value>) -> (u16, Value) {
    let mut rb = reqwest::Client::new().request(method, format!("{url}{path}")).header("Group-Auth", auth);
    if let Some(b) = body {
        rb = rb.json(&b);
    }
    let r = rb.send().await.unwrap();
    let status = r.status().as_u16();
    (status, r.json().await.unwrap_or(Value::Null))
}

/// The server's authorization of the v2 API, with real credentials: non-members, members
/// without the role, wrong group parameters and stale revisions are refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn v2_server_authorization() {
    let Some(url) = v2_server().await else { return };
    let (ana, bob, eve) = (Client::new("ana"), Client::new("bob"), Client::new("eve"));
    let acc = register_all(&url, &[&ana, &bob, &eve]).await;
    let params = GroupSecretParams::derive_from_master_key(GroupMasterKey::new(chat_core::crypto::random_bytes::<32>()));
    let other = GroupSecretParams::derive_from_master_key(GroupMasterKey::new(chat_core::crypto::random_bytes::<32>()));
    let (a, b, e) = (&acc[0].account_id, &acc[1].account_id, &acc[2].account_id);
    let ana_auth = group_auth(&params, &presentation(&ana, params).await);
    let bob_auth = group_auth(&params, &presentation(&bob, params).await);
    let eve_auth = group_auth(&params, &presentation(&eve, params).await);
    let blob = b64(params.encrypt_blob(chat_core::crypto::random_bytes::<32>(), b"x"));
    use reqwest::Method;

    // Only a creator listed as admin can create; then it exists once.
    let create = |admin: &str| {
        json!({ "attributes": blob, "settings": blob, "members": [
            { "uid": uid(&params, a), "role": if admin == "ana" { "admin" } else { "member" } },
            { "uid": uid(&params, b), "role": "member" },
        ] })
    };
    assert_eq!(call(&url, Method::POST, "/v2/groups", &ana_auth, Some(create("nobody"))).await.0, 403);
    assert_eq!(call(&url, Method::POST, "/v2/groups", &eve_auth, Some(create("ana"))).await.0, 403);
    let (s, r) = call(&url, Method::POST, "/v2/groups", &ana_auth, Some(create("ana"))).await;
    assert_eq!(s, 200, "{r}");
    assert_eq!(call(&url, Method::POST, "/v2/groups", &ana_auth, Some(create("ana"))).await.0, 409);

    // Members read the state; a non-member's presentation is refused.
    let (s, st) = call(&url, Method::GET, "/v2/groups", &bob_auth, None).await;
    assert_eq!(s, 200);
    assert_eq!(st["members"].as_array().unwrap().len(), 2);
    assert_eq!(call(&url, Method::GET, "/v2/groups", &eve_auth, None).await.0, 403);
    assert_eq!(call(&url, Method::GET, "/v2/groups/changes?since=0", &eve_auth, None).await.0, 403);
    let add_eve = json!({ "revision": 2, "actions": [{ "type": "add_member", "uid": uid(&params, e), "role": "member" }] });
    assert_eq!(call(&url, Method::PATCH, "/v2/groups", &eve_auth, Some(add_eve.clone())).await.0, 403);

    // Wrong group parameters (a presentation for another group, or no credential at all).
    let (_, pres) = ana_auth.split_once(':').unwrap();
    let swapped = format!("{}:{pres}", b64(zkgroup::serialize(&other.get_public_params())));
    assert_eq!(call(&url, Method::GET, "/v2/groups", &swapped, None).await.0, 401);
    assert_eq!(call(&url, Method::GET, "/v2/groups", "garbage", None).await.0, 401);
    let other_auth = group_auth(&other, &presentation(&ana, other).await);
    assert_eq!(call(&url, Method::GET, "/v2/groups", &other_auth, None).await.0, 404);

    // A member who is not an admin cannot add or remove.
    assert_eq!(call(&url, Method::PATCH, "/v2/groups", &bob_auth, Some(add_eve.clone())).await.0, 403);
    let remove_ana = json!({ "revision": 2, "actions": [{ "type": "remove_member", "uid": uid(&params, a) }] });
    assert_eq!(call(&url, Method::PATCH, "/v2/groups", &bob_auth, Some(remove_ana)).await.0, 403);

    // The admin can; a stale revision is a conflict that names the current one.
    let stale = json!({ "revision": 7, "actions": add_eve["actions"] });
    let (s, r) = call(&url, Method::PATCH, "/v2/groups", &ana_auth, Some(stale)).await;
    assert_eq!((s, r["revision"].as_i64()), (409, Some(1)));
    let (s, r) = call(&url, Method::PATCH, "/v2/groups", &ana_auth, Some(add_eve)).await;
    assert_eq!(s, 200, "{r}");
    assert_eq!(call(&url, Method::GET, "/v2/groups", &eve_auth, None).await.0, 200);

    // Pending invitation: the invitee alone accepts it.
    let remove_bob = json!({ "revision": 3, "actions": [{ "type": "remove_member", "uid": uid(&params, b) }] });
    assert_eq!(call(&url, Method::PATCH, "/v2/groups", &ana_auth, Some(remove_bob)).await.0, 200);
    let invite_bob = json!({ "revision": 4, "actions": [{ "type": "add_pending", "uid": uid(&params, b), "role": "member" }] });
    assert_eq!(call(&url, Method::PATCH, "/v2/groups", &ana_auth, Some(invite_bob)).await.0, 200);
    let accept = json!({ "revision": 5, "actions": [{ "type": "accept_pending" }] });
    assert_eq!(call(&url, Method::PATCH, "/v2/groups", &eve_auth, Some(accept.clone())).await.0, 403);
    assert_eq!(call(&url, Method::PATCH, "/v2/groups", &bob_auth, Some(accept)).await.0, 200);
    let (_, st) = call(&url, Method::GET, "/v2/groups", &bob_auth, None).await;
    assert_eq!(st["revision"], 5);
    assert_eq!(st["members"].as_array().unwrap().len(), 3);
    assert!(st["pending"].as_array().unwrap().is_empty());
}
