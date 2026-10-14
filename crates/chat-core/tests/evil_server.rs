//! Compromised-server test: the server swaps an account's identity key (as if it injected a
//! device of its own). Clients must refuse to encrypt for it instead of trusting the change.
//!
//! Needs the server and direct access to its MySQL database (to play the attacker), e.g.:
//! `CHATAPP_E2E_SERVER=http://127.0.0.1:8080 CHATAPP_E2E_MYSQL="-uchatapp -pchatapp chatapp_dev"
//!  cargo test -p chat-core --test evil_server -- --nocapture`

use std::sync::{Arc, Mutex};
use std::time::Duration;

use chat_core::model::*;
use chat_core::engine::CallSignalRequest;
use chat_core::{Core, CoreConfig, EventSink};
use serde_json::Value;

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

    /// Waits until an event matching `pred` was emitted.
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

    fn saw_message(&self, text: &str) -> bool {
        self.events
            .0
            .lock()
            .unwrap()
            .iter()
            .any(|(n, v)| n == "message" && v["message"]["text"].as_str() == Some(text))
    }

    async fn wait_online(&self) {
        self.wait_event("online", |n, v| n == "connection" && v.as_str() == Some("online")).await;
    }
}

fn server() -> String {
    std::env::var("CHATAPP_E2E_SERVER").unwrap_or_default()
}

async fn send_text(c: &Client, conv: &str, text: &str) -> Message {
    let req: SendMessageRequest = serde_json::from_value(serde_json::json!({
        "conversation_id": conv, "text": text
    }))
    .unwrap();
    c.core.run(move |i| async move { i.send_message(req).await }).await.unwrap()
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn compromised_server_cannot_swap_identity() {
    if server().is_empty() || std::env::var("CHATAPP_E2E_MYSQL").is_err() {
        eprintln!("CHATAPP_E2E_SERVER / CHATAPP_E2E_MYSQL not set, skipping");
        return;
    }
    let _ = env_logger::builder().try_init();
    let url = server();
    let (alice, bob, mallory) = (Client::new("alice"), Client::new("bob"), Client::new("mallory"));
    let mut accounts = Vec::new();
    for c in [&alice, &bob, &mallory] {
        let u = url.clone();
        let r = c.core.run(move |i| async move { i.register(&u, "correct horse battery", "test device").await }).await.unwrap();
        accounts.push(r.account);
        c.wait_online().await;
    }
    let (b_acc, m_acc) = (accounts[1].clone(), accounts[2].clone());

    // Alice and Bob talk: Alice pins Bob's identity.
    let bnum = b_acc.number.clone();
    let contact = alice.core.run(move |i| async move { i.add_contact_by_number(&bnum, None).await }).await.unwrap();
    let bid = contact.account_id.clone();
    let conv = alice.core.run(move |i| async move { i.open_direct_conversation(&bid).await }).await.unwrap();
    send_text(&alice, &conv.id, "hello bob").await;
    bob.wait_message("hello bob").await;

    // The server is compromised: Bob's account now advertises Mallory's identity key (with
    // keys Mallory controls, the attacker could then read whatever is encrypted for it).
    // The bundle the server now serves for Bob is Mallory's, complete and validly signed by
    // Mallory's identity: only identity pinning stands between Alice and the attacker.
    let (b, bd, m, md) = (&b_acc.account_id, b_acc.device_id, &m_acc.account_id, m_acc.device_id);
    let original = sql(&format!("SELECT HEX(identity_key) FROM accounts WHERE id = '{b}'"));
    let backup = format!("evil_backup_{}", &b[..8]);
    sql(&format!(
        "CREATE TABLE {backup}_s AS SELECT * FROM device_signed_keys WHERE account_id = '{b}' AND device_id = {bd}; \
         CREATE TABLE {backup}_o AS SELECT * FROM device_one_time_keys WHERE account_id = '{b}' AND device_id = {bd}; \
         UPDATE accounts SET identity_key = (SELECT k FROM (SELECT identity_key AS k FROM accounts WHERE id = '{m}') t) WHERE id = '{b}'; \
         DELETE FROM device_signed_keys WHERE account_id = '{b}' AND device_id = {bd}; \
         DELETE FROM device_one_time_keys WHERE account_id = '{b}' AND device_id = {bd}; \
         INSERT INTO device_signed_keys SELECT '{b}', {bd}, kind, key_id, public_key, signature FROM device_signed_keys WHERE account_id = '{m}' AND device_id = {md}; \
         INSERT INTO device_one_time_keys SELECT '{b}', {bd}, kind, key_id, public_key, signature FROM device_one_time_keys WHERE account_id = '{m}' AND device_id = {md};"
    ));
    // Alice has to fetch Bob's keys again (what a newly added device triggers).
    let bid = b.clone();
    alice.core.run(move |i| async move { i.delete_session(&bid, bd) }).await.unwrap();

    let refused = send_text(&alice, &conv.id, "secret for bob only").await;
    alice.wait_status(&refused.id, "failed").await;
    let bid = b.clone();
    alice
        .wait_event("identity conflict", move |n, v| n == "identity_conflict" && v["account_id"].as_str() == Some(bid.as_str()))
        .await;
    let cid = conv.id.clone();
    let msgs = alice.core.run(move |i| async move { i.get_messages(&cid, None, Some(50)) }).await.unwrap();
    assert!(
        msgs.iter().any(|m| m.system.as_ref().is_some_and(|s| s.kind == "identity_mismatch")),
        "the conversation must show the warning"
    );
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(!bob.saw_message("secret for bob only"), "nothing may be encrypted for the swapped key");

    // The server is fixed: messages flow again with the pinned key.
    sql(&format!(
        "UPDATE accounts SET identity_key = UNHEX('{original}') WHERE id = '{b}'; \
         DELETE FROM device_signed_keys WHERE account_id = '{b}' AND device_id = {bd}; \
         DELETE FROM device_one_time_keys WHERE account_id = '{b}' AND device_id = {bd}; \
         INSERT INTO device_signed_keys SELECT * FROM {backup}_s; INSERT INTO device_one_time_keys SELECT * FROM {backup}_o; \
         DROP TABLE {backup}_s; DROP TABLE {backup}_o;"
    ));
    let bid = b.clone();
    alice.core.run(move |i| async move { i.delete_session(&bid, bd) }).await.unwrap();
    send_text(&alice, &conv.id, "all good again").await;
    bob.wait_message("all good again").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn compromised_server_cannot_inject_a_device() {
    if server().is_empty() || std::env::var("CHATAPP_E2E_MYSQL").is_err() {
        eprintln!("CHATAPP_E2E_SERVER / CHATAPP_E2E_MYSQL not set, skipping");
        return;
    }
    let _ = env_logger::builder().try_init();
    let url = server();
    let (alice, bob, mallory) = (Client::new("alice"), Client::new("bob"), Client::new("mallory"));
    let mut accounts = Vec::new();
    for c in [&alice, &bob, &mallory] {
        let u = url.clone();
        let r = c.core.run(move |i| async move { i.register(&u, "correct horse battery", "test device").await }).await.unwrap();
        accounts.push(r.account);
        c.wait_online().await;
    }
    let (b_acc, m_acc) = (accounts[1].clone(), accounts[2].clone());
    let bnum = b_acc.number.clone();
    let contact = alice.core.run(move |i| async move { i.add_contact_by_number(&bnum, None).await }).await.unwrap();
    let bid = contact.account_id.clone();
    let conv = alice.core.run(move |i| async move { i.open_direct_conversation(&bid).await }).await.unwrap();
    send_text(&alice, &conv.id, "hello bob").await;
    bob.wait_message("hello bob").await;

    // The server adds a device of the attacker to Bob's account (Mallory's keys, and Bob's
    // account identity swapped so the device's signatures check out).
    let (b, m, md) = (&b_acc.account_id, &m_acc.account_id, m_acc.device_id);
    let original = sql(&format!("SELECT HEX(identity_key) FROM accounts WHERE id = '{b}'"));
    sql(&format!(
        "UPDATE accounts SET identity_key = (SELECT k FROM (SELECT identity_key AS k FROM accounts WHERE id = '{m}') t) WHERE id = '{b}'; \
         INSERT INTO devices (account_id, device_id, name, platform, registration_id, token_hash, created_at, last_seen_at) \
           VALUES ('{b}', 9, 'injected', 'desktop', 4242, UNHEX(SHA2('{b}-injected', 256)), 0, 0); \
         INSERT INTO device_signed_keys SELECT '{b}', 9, kind, key_id, public_key, signature FROM device_signed_keys WHERE account_id = '{m}' AND device_id = {md}; \
         INSERT INTO device_one_time_keys SELECT '{b}', 9, kind, key_id, public_key, signature FROM device_one_time_keys WHERE account_id = '{m}' AND device_id = {md};"
    ));

    let refused = send_text(&alice, &conv.id, "secret for bob only").await;
    alice.wait_status(&refused.id, "failed").await;
    let bid = b.clone();
    alice
        .wait_event("identity conflict", move |n, v| n == "identity_conflict" && v["account_id"].as_str() == Some(bid.as_str()))
        .await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(!bob.saw_message("secret for bob only"), "nothing may be sent while the account has a foreign device");
    let queued: i64 = sql(&format!("SELECT COUNT(*) FROM envelopes WHERE account_id = '{b}' AND device_id = 9"))
        .parse()
        .unwrap_or(0);
    assert_eq!(queued, 0, "no envelope may be encrypted for the injected device");

    sql(&format!("DELETE FROM devices WHERE account_id = '{b}' AND device_id = 9; UPDATE accounts SET identity_key = UNHEX('{original}') WHERE id = '{b}';"));
    send_text(&alice, &conv.id, "all good again").await;
    bob.wait_message("all good again").await;
}

/// Sealed sender: once the parties know each other's profile key, the server stores 1:1
/// messages without a sender, and still delivers them.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn server_does_not_learn_the_sender() {
    if server().is_empty() || std::env::var("CHATAPP_E2E_MYSQL").is_err() {
        eprintln!("CHATAPP_E2E_SERVER / CHATAPP_E2E_MYSQL not set, skipping");
        return;
    }
    let _ = env_logger::builder().try_init();
    let url = server();
    let (alice, bob) = (Client::new("alice"), Client::new("bob"));
    let mut accounts = Vec::new();
    for c in [&alice, &bob] {
        let u = url.clone();
        let r = c.core.run(move |i| async move { i.register(&u, "correct horse battery", "test device").await }).await.unwrap();
        accounts.push(r.account);
        c.wait_online().await;
    }
    let (a, b) = (accounts[0].clone(), accounts[1].clone());
    // First contact: Alice does not know Bob's profile key yet, so this one is identified.
    let n = b.number.clone();
    let contact = alice.core.run(move |i| async move { i.add_contact_by_number(&n, None).await }).await.unwrap();
    let conv = alice.core.run(move |i| async move { i.open_direct_conversation(&contact.account_id).await }).await.unwrap();
    send_text(&alice, &conv.id, "hello bob").await;
    let bob_conv = bob.wait_message("hello bob").await["conversation_id"].as_str().unwrap().to_string();
    send_text(&bob, &bob_conv, "hello alice").await;
    alice.wait_message("hello alice").await;
    tokio::time::sleep(Duration::from_secs(1)).await;

    // Alice goes offline; whatever Bob sends now waits on the server.
    alice.core.run(|i| async move { i.stop_background(); Ok(()) }).await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    let m = send_text(&bob, &bob_conv, "who sent this?").await;
    bob.wait_status(&m.id, "sent").await;
    let stored = sql(&format!("SELECT type, source_account, source_device FROM envelopes WHERE account_id = '{}'", a.account_id));
    eprintln!("stored for alice:\n{stored}");
    assert!(!stored.is_empty(), "the message should wait on the server");
    for line in stored.lines() {
        let cols: Vec<&str> = line.split('\t').collect();
        assert_eq!(cols[0], "6", "not sealed: {line}");
        assert!(cols[1].is_empty() && cols[2] == "0", "the server knows the sender: {line}");
        assert!(!line.contains(&b.account_id));
    }
    // Alice reconnects and gets it from Bob, as usual.
    alice.core.run(|i| async move { i.clone().start_background(); Ok(()) }).await.unwrap();
    let got = alice.wait_message("who sent this?").await;
    assert_eq!(got["conversation_id"].as_str(), Some(conv.id.as_str()));
    assert_eq!(got["sender_account_id"].as_str(), Some(b.account_id.as_str()));

    // Alice now knows Bob's key too: her reply is sealed as well.
    bob.core.run(|i| async move { i.stop_background(); Ok(()) }).await.unwrap();
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let m = send_text(&alice, &conv.id, "sealed reply").await;
    alice.wait_status(&m.id, "sent").await;
    let stored = sql(&format!("SELECT type, source_account FROM envelopes WHERE account_id = '{}'", b.account_id));
    assert!(!stored.is_empty() && stored.lines().all(|l| l.split('\t').next() == Some("6") && !l.contains(&a.account_id)), "reply not sealed:\n{stored}");
    bob.core.run(|i| async move { i.clone().start_background(); Ok(()) }).await.unwrap();
    bob.wait_message("sealed reply").await;

    // A refused access key (e.g. changed) falls back to identified delivery.
    sql(&format!("UPDATE accounts SET unidentified_access_key = NULL WHERE id = '{}'", b.account_id));
    send_text(&alice, &conv.id, "after the key changed").await;
    bob.wait_message("after the key changed").await;
}

/// The audit test: a secret string goes through every feature of the chat while the
/// recipients are offline (so everything waits on the server), then the whole server side is
/// searched for it: the full database dump, every object file of the attachment storage and
/// the server log. The recipients then come online and must read everything correctly.
///
/// Extra env: CHATAPP_E2E_MINIO_DIR (MinIO data directory), CHATAPP_E2E_SERVER_LOG.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn nothing_readable_reaches_the_server() {
    if server().is_empty() || std::env::var("CHATAPP_E2E_MYSQL").is_err() {
        eprintln!("CHATAPP_E2E_SERVER / CHATAPP_E2E_MYSQL not set, skipping");
        return;
    }
    use base64::Engine as _;
    let _ = env_logger::builder().try_init();
    let url = server();
    let canary = format!("CANARY-{}", hex::encode(rand::random::<[u8; 8]>()));
    let (alice, bob, carol) = (Client::new("alice"), Client::new("bob"), Client::new("carol"));
    let mut accounts = Vec::new();
    for c in [&alice, &bob, &carol] {
        let u = url.clone();
        let r = c.core.run(move |i| async move { i.register(&u, "correct horse battery", "test device").await }).await.unwrap();
        accounts.push(r.account);
        c.wait_online().await;
    }
    let (b, c) = (accounts[1].clone(), accounts[2].clone());
    // Profile name and about are end-to-end encrypted too.
    let pn = format!("Alice {canary}");
    alice
        .core
        .run(move |i| async move {
            i.set_profile(SetProfileRequest { name: Some(pn.clone()), about: Some(pn), avatar_path: None, avatar_data_base64: None }).await
        })
        .await
        .unwrap();
    // Sessions and profile keys first (so the later 1:1 messages are sealed as well).
    let mut convs = Vec::new();
    for peer in [&b, &c] {
        let n = peer.number.clone();
        let contact = alice.core.run(move |i| async move { i.add_contact_by_number(&n, None).await }).await.unwrap();
        convs.push(alice.core.run(move |i| async move { i.open_direct_conversation(&contact.account_id).await }).await.unwrap());
    }
    // Alice is a contact of both (a stranger's calls never reach the UI: message requests).
    for x in [&bob, &carol] {
        let n = accounts[0].number.clone();
        x.core.run(move |i| async move { i.add_contact_by_number(&n, None).await }).await.unwrap();
    }
    send_text(&alice, &convs[0].id, "hi bob").await;
    let bob_conv = bob.wait_message("hi bob").await["conversation_id"].as_str().unwrap().to_string();
    send_text(&bob, &bob_conv, "hi alice").await;
    alice.wait_message("hi alice").await;

    // Recipients offline from here on.
    for x in [&bob, &carol] {
        x.core.run(|i| async move { i.stop_background(); Ok(()) }).await.unwrap();
    }
    tokio::time::sleep(Duration::from_millis(800)).await;
    let conv = convs[0].id.clone();
    let b64 = |d: &[u8]| base64::engine::general_purpose::STANDARD.encode(d);
    let send = |req: serde_json::Value| {
        let r: SendMessageRequest = serde_json::from_value(req).unwrap();
        alice.core.run(move |i| async move { i.send_message(r).await })
    };
    let mut sent = Vec::new();
    // Text, reply, edit.
    let text = send(serde_json::json!({ "conversation_id": conv, "text": format!("text {canary}") })).await.unwrap();
    sent.push(send(serde_json::json!({ "conversation_id": conv, "text": format!("reply {canary}"), "reply_to": text.id })).await.unwrap());
    let (tid, et) = (text.id.clone(), format!("edited {canary}"));
    alice.core.run(move |i| async move { i.edit_message(&tid, &et, &[]) }).await.unwrap();
    // Image (the secret inside the pixels' file), document (in the bytes and the name), voice.
    let mut png = Vec::new();
    image::RgbImage::from_fn(32, 32, |x, y| image::Rgb([x as u8 * 8, y as u8 * 8, 99]))
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    png.extend_from_slice(canary.as_bytes());
    let doc: Vec<u8> = canary.as_bytes().iter().cycle().take(1000).copied().collect();
    let mut wav = vec![0u8; 4000];
    wav.extend_from_slice(canary.as_bytes());
    sent.push(
        send(serde_json::json!({ "conversation_id": conv, "text": format!("files {canary}"), "attachments": [
            { "data_base64": b64(&png), "file_name": format!("{canary}.png"), "mime": "image/png", "kind": "image" },
            { "data_base64": b64(&doc), "file_name": format!("{canary}.txt"), "mime": "text/plain", "kind": "file" },
            { "data_base64": b64(&wav), "file_name": "voice.wav", "mime": "audio/wav", "kind": "voice", "duration_ms": 1000, "waveform": [0.5] }
        ]}))
        .await
        .unwrap(),
    );
    // Location and a contact card.
    sent.push(
        send(serde_json::json!({ "conversation_id": conv, "location": { "lat": 44.4268, "lng": 26.1025, "name": format!("place {canary}"), "address": canary } }))
            .await
            .unwrap(),
    );
    sent.push(send(serde_json::json!({ "conversation_id": conv, "contact_ids": [c.account_id] })).await.unwrap());
    // Call signaling (the SDP carries the secret here).
    let call = CallSignalRequest {
        account_id: b.account_id.clone(),
        call_id: "call-1".into(),
        kind: "offer".into(),
        // A well-formed offer (the core drops anything else), the secret as its session name.
        data: serde_json::json!({ "sdp": format!(
            "v=0\r\no=- 1 2 IN IP4 127.0.0.1\r\ns={canary}\r\nt=0 0\r\nm=audio 9 UDP/TLS/RTP/SAVPF 111\r\nc=IN IP4 0.0.0.0\r\n\
             a=fingerprint:sha-256 4A:AD:B9:B1:3F:82:18:3B:54:02:12:DF:3E:5D:49:6B:19:E5:7C:AB:3D:B4:0A:3E:4E:A9:66:9F:12:E5:8A:01\r\n\
             a=setup:actpass\r\na=mid:0\r\na=rtpmap:111 opus/48000/2\r\n"
        ) }),
        target_device: None,
        to_own_devices: false,
    };
    alice.core.run(move |i| async move { i.call_signal(call).await }).await.unwrap();
    // Typing.
    let tc = conv.clone();
    alice.core.run(move |i| async move { i.send_typing(&tc, true).await }).await.unwrap();
    // A group: name, description and a message.
    let members = vec![b.account_id.clone(), c.account_id.clone()];
    let greq: CreateGroupRequest = serde_json::from_value(
        serde_json::json!({ "name": format!("group {canary}"), "description": canary, "member_account_ids": members }),
    )
    .unwrap();
    let group = alice.core.run(move |i| async move { i.create_group(greq).await }).await.unwrap();
    sent.push(send(serde_json::json!({ "conversation_id": group.id, "text": format!("group text {canary}") })).await.unwrap());
    // Its invite link: the preview blob holds the name, description and group key.
    let ireq = chat_core::engine::invites::SetInviteRequest { conversation_id: group.id.clone(), enabled: true, approval: Some(false), reset: false };
    let invite = alice.core.run(move |i| async move { i.set_group_invite(ireq).await }).await.unwrap();
    assert!(invite.invite_link.is_some());
    for m in &sent {
        alice.wait_status(&m.id, "sent").await;
    }
    tokio::time::sleep(Duration::from_secs(4)).await; // vault backup flushes too

    // --- the server's side ---------------------------------------------------------
    let mut needles: Vec<(String, Vec<u8>)> = vec![("raw".into(), canary.as_bytes().to_vec())];
    for shift in 0..3 {
        let mut d = vec![b'x'; shift];
        d.extend_from_slice(canary.as_bytes());
        let e = b64(&d);
        needles.push((format!("base64/{shift}"), e.as_bytes()[4..e.len() - 4].to_vec()));
    }
    needles.push(("hex".into(), hex::encode(canary.as_bytes()).into_bytes()));
    let find = |what: &str, data: &[u8]| {
        for (kind, n) in &needles {
            assert!(!data.windows(n.len()).any(|w| w == n.as_slice()), "the secret is readable in {what} ({kind})");
        }
    };
    let args = std::env::var("CHATAPP_E2E_MYSQL").unwrap();
    let dump = std::process::Command::new("mysqldump").args(args.split_whitespace()).output().expect("mysqldump");
    assert!(dump.status.success() && dump.stdout.len() > 10_000, "mysqldump failed");
    let envelopes = sql(&format!("SELECT COUNT(*) FROM envelopes WHERE account_id IN ('{}','{}')", b.account_id, c.account_id));
    eprintln!("database dump: {} bytes, {envelopes} envelopes waiting for bob/carol", dump.stdout.len());
    assert!(envelopes.parse::<i64>().unwrap() >= 8);
    find("the database", &dump.stdout);
    // Control: the same search does see what the server legitimately stores in clear.
    let number = accounts[0].number.as_bytes();
    assert!(dump.stdout.windows(number.len()).any(|w| w == number), "the dump search does not work");
    if let Ok(dir) = std::env::var("CHATAPP_E2E_MINIO_DIR") {
        let mut files = 0;
        let mut stack = vec![std::path::PathBuf::from(dir)];
        while let Some(p) = stack.pop() {
            for e in std::fs::read_dir(&p).unwrap().flatten() {
                if e.file_type().unwrap().is_dir() {
                    stack.push(e.path());
                } else {
                    files += 1;
                    find(&format!("{}", e.path().display()), &std::fs::read(e.path()).unwrap());
                }
            }
        }
        eprintln!("attachment storage: {files} files checked");
    }
    if let Ok(log) = std::env::var("CHATAPP_E2E_SERVER_LOG") {
        find("the server log", &std::fs::read(log).unwrap());
    }
    // Attachment sizes are padded: the stored size is not the file's size plus fixed overhead.
    let sizes = sql(&format!("SELECT size FROM attachments WHERE owner_id = '{}' AND kind = 'message'", accounts[0].account_id));
    let exact = |n: usize| (16 + (n / 16 + 1) * 16 + 32).to_string();
    for s in sizes.lines() {
        assert!(s != exact(doc.len()) && s != exact(png.len()) && s != exact(wav.len()), "unpadded attachment size {s}");
    }

    // --- and the recipients still read everything -----------------------------------
    for x in [&bob, &carol] {
        x.core.run(|i| async move { i.clone().start_background(); Ok(()) }).await.unwrap();
    }
    bob.wait_message(&format!("reply {canary}")).await;
    let files = bob.wait_message(&format!("files {canary}")).await;
    assert_eq!(files["attachments"].as_array().unwrap().len(), 3);
    bob.wait_message(&format!("group text {canary}")).await;
    carol.wait_message(&format!("group text {canary}")).await;
    bob.wait_event("edit", |n, v| n == "message" && v["message"]["text"].as_str().is_some_and(|t| t.starts_with("edited "))).await;
    bob.wait_event("location", |n, v| n == "message" && v["message"]["location"]["address"].is_string()).await;
    bob.wait_event("call", |n, v| n == "call_signal" && v["kind"] == "offer").await;
    // Downloaded files are byte-identical (padding removed).
    let mid = files["id"].as_str().unwrap().to_string();
    for att in files["attachments"].as_array().unwrap() {
        let (m, a) = (mid.clone(), att["id"].as_str().unwrap().to_string());
        bob.core.run(move |i| async move { i.download_attachment(&m, &a).await }).await.unwrap();
    }
    let mid2 = mid.clone();
    let done = bob
        .wait_event("downloads", move |n, v| {
            n == "message"
                && v["message"]["id"].as_str() == Some(mid2.as_str())
                && v["message"]["attachments"].as_array().is_some_and(|a| a.iter().all(|x| x["download_state"] == "done"))
        })
        .await;
    // Photos lose their metadata before sending (trailing bytes after the PNG end included).
    let png = chat_core::sanitize::strip_png_metadata(&png).unwrap();
    for (att, original) in done["message"]["attachments"].as_array().unwrap().iter().zip([&png, &doc, &wav]) {
        let path = att["local_path"].as_str().unwrap().to_string();
        // On the recipient's disk the file is encrypted too; the core decrypts it.
        find(&format!("bob's disk ({path})"), &std::fs::read(&path).unwrap());
        let got = bob.core.run(move |i| async move { i.read_media_file(std::path::Path::new(&path)).await }).await.unwrap();
        assert_eq!(&got, original, "attachment {} differs after the round trip", att["file_name"]);
    }
}
