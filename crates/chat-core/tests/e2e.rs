//! End-to-end test against a real server.
//!
//! Run with a server listening (rate limits disabled), e.g.:
//! `CHATAPP_E2E_SERVER=http://127.0.0.1:8080 cargo test -p chat-core --test e2e -- --nocapture`

use std::sync::{Arc, Mutex};
use std::time::Duration;

use chat_core::model::*;
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
    _dir: Option<tempfile::TempDir>,
}

impl Client {
    fn new(name: &'static str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut c = Self::at(name, dir.path());
        c._dir = Some(dir);
        c
    }

    /// A client on a given data directory (to restart it on the same data).
    fn at(name: &'static str, dir: &std::path::Path) -> Self {
        let events = Arc::new(Events::default());
        let core = Core::start(CoreConfig {
            data_dir: dir.to_path_buf(),
            db_key: [7u8; 32],
            default_server_url: server(),
            platform: "cli".into(),
            events: events.clone(),
        })
        .unwrap();
        Self { name, core, events, _dir: None }
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn full_flow() {
    if server().is_empty() {
        eprintln!("CHATAPP_E2E_SERVER not set, skipping");
        return;
    }
    let _ = env_logger::builder().try_init();
    let url = server();

    // --- registration ------------------------------------------------------------
    let alice = Client::new("alice");
    let bob = Client::new("bob");
    let carol = Client::new("carol");
    let mut accounts = Vec::new();
    let mut bob_mnemonic = Vec::new();
    for c in [&alice, &bob, &carol] {
        let u = url.clone();
        let r = c
            .core
            .run(move |i| async move { i.register(&u, "correct horse battery", "test device").await })
            .await
            .unwrap();
        assert_eq!(r.mnemonic.len(), 24);
        assert_eq!(r.account.number.len(), 16);
        if c.name == "bob" {
            bob_mnemonic = r.mnemonic.clone();
        }
        accounts.push(r.account);
        c.wait_online().await;
    }
    let (a_acc, b_acc, c_acc) = (accounts[0].clone(), accounts[1].clone(), accounts[2].clone());

    // The recovery phrase can be shown again with the password (and only with it).
    let words = bob.core.run(|i| async move { i.reveal_mnemonic("correct horse battery").await }).await.unwrap();
    assert_eq!(words, bob_mnemonic);
    assert!(bob.core.run(|i| async move { i.reveal_mnemonic("wrong password!").await }).await.is_err());

    // Profiles.
    alice
        .core
        .run(|i| async move {
            i.set_profile(SetProfileRequest { name: Some("Alice".into()), about: Some("hi".into()), avatar_path: None, avatar_data_base64: None })
                .await
        })
        .await
        .unwrap();

    // --- 1:1 text, receipts ------------------------------------------------------
    let bnum = b_acc.number.clone();
    let contact = alice.core.run(move |i| async move { i.add_contact_by_number(&bnum, Some("Bobby".into())).await }).await.unwrap();
    assert_eq!(contact.account_id, b_acc.account_id);
    assert_eq!(contact.display_name, "Bobby");
    let bid = b_acc.account_id.clone();
    let conv = alice.core.run(move |i| async move { i.open_direct_conversation(&bid).await }).await.unwrap();
    let sent = send_text(&alice, &conv.id, "Hello Bob!").await;
    assert_eq!(sent.status, "pending");
    let received = bob.wait_message("Hello Bob!").await;
    let bob_conv = received["conversation_id"].as_str().unwrap().to_string();
    // Alice is not in Bob's contacts: a message request, acknowledged once Bob accepts it.
    let bc = bob_conv.clone();
    let request = bob.core.run(move |i| async move { i.get_conversation(&bc) }).await.unwrap();
    assert!(!request.accepted && !request.peer_is_contact);
    let bc = bob_conv.clone();
    assert!(bob.core.run(move |i| async move { i.accept_conversation(&bc).await }).await.unwrap().accepted);
    alice.wait_status(&sent.id, "delivered").await;
    // Bob learned Alice's name from her profile key.
    bob.wait_event("alice profile", |n, v| n == "contact_updated" && v["profile_name"].as_str() == Some("Alice")).await;

    let bc = bob_conv.clone();
    bob.core.run(move |i| async move { i.mark_read(&bc) }).await.unwrap();
    alice.wait_status(&sent.id, "read").await;

    // --- reply, reaction, edit, delete ---------------------------------------------
    let req: SendMessageRequest = serde_json::from_value(serde_json::json!({
        "conversation_id": bob_conv, "text": "Hi Alice", "reply_to": received["id"]
    }))
    .unwrap();
    let reply = bob.core.run(move |i| async move { i.send_message(req).await }).await.unwrap();
    let got = alice.wait_message("Hi Alice").await;
    assert_eq!(got["reply_to"]["id"].as_str(), Some(sent.id.as_str()));

    let rid = got["id"].as_str().unwrap().to_string();
    alice.core.run(move |i| async move { i.react(&rid, Some("👍".into())) }).await.unwrap();
    let reply_id = reply.id.clone();
    bob.wait_event("reaction", move |n, v| {
        n == "message" && v["message"]["id"].as_str() == Some(reply_id.as_str()) && v["message"]["reactions"][0]["emoji"].as_str() == Some("👍")
    })
    .await;

    let sid = sent.id.clone();
    alice.core.run(move |i| async move { i.edit_message(&sid, "Hello Bob! (edited)", &[]) }).await.unwrap();
    let edited = bob.wait_message("Hello Bob! (edited)").await;
    let eid = edited["id"].as_str().unwrap().to_string();
    let history = bob.core.run(move |i| async move { i.get_edit_history(&eid) }).await.unwrap();
    let texts: Vec<&str> = history.iter().map(|v| v.text.as_str()).collect();
    assert_eq!(texts, ["Hello Bob!", "Hello Bob! (edited)"], "edit history");
    let sid = sent.id.clone();
    alice.core.run(move |i| async move { i.delete_message(&sid, true) }).await.unwrap();
    let sid = sent.id.clone();
    bob.wait_event("delete", move |n, v| n == "message" && v["message"]["id"].as_str() == Some(sid.as_str()) && v["message"]["deleted"] == true).await;

    // --- disappearing messages survive vault sync (regression) -------------------------
    let (cid, bc2) = (conv.id.clone(), bob_conv.clone());
    alice
        .core
        .run(move |i| async move {
            i.set_conversation_settings(serde_json::from_value(serde_json::json!({
                "conversation_id": cid, "disappearing_seconds": 86400
            })).unwrap()).await
        })
        .await
        .unwrap();
    let temp = send_text(&alice, &conv.id, "temporary message").await;
    assert!(temp.expires_at.is_some());
    bob.wait_message("temporary message").await;
    tokio::time::sleep(Duration::from_secs(4)).await; // several vault flush/pull cycles
    for (c, conv_id) in [(&alice, conv.id.clone()), (&bob, bc2.clone())] {
        let msgs = c.core.run(move |i| async move { i.get_messages(&conv_id, None, Some(50)) }).await.unwrap();
        assert!(msgs.iter().any(|m| m.text.as_deref() == Some("temporary message")), "{}: disappearing message vanished", c.name);
    }
    // A voice note in a disappearing conversation (regression: stuck as pending on the sender).
    let wav: Vec<u8> = (0..32_000u32).map(|i| (i % 200) as u8).collect();
    let req: SendMessageRequest = serde_json::from_value(serde_json::json!({
        "conversation_id": conv.id,
        "attachments": [{ "data_base64": base64_encode(&wav), "file_name": "voice.wav", "mime": "audio/wav",
            "kind": "voice", "duration_ms": 1000, "waveform": [0.2, 0.8, 0.5] }]
    }))
    .unwrap();
    let voice = alice.core.run(move |i| async move { i.send_message(req).await }).await.unwrap();
    assert!(voice.expires_at.is_some());
    alice.wait_status(&voice.id, "delivered").await;
    let vid = voice.id.clone();
    bob.wait_event("disappearing voice note", move |n, v| {
        n == "message" && v["message"]["id"].as_str() == Some(vid.as_str()) && v["message"]["attachments"][0]["download_state"] == "done"
    })
    .await;
    let cid = conv.id.clone();
    alice
        .core
        .run(move |i| async move {
            i.set_conversation_settings(serde_json::from_value(serde_json::json!({
                "conversation_id": cid, "disappearing_seconds": 0
            })).unwrap()).await
        })
        .await
        .unwrap();

    // --- profile changes reach contacts; usernames; directory search ---------------------
    bob.core
        .run(|i| async move {
            i.set_profile(SetProfileRequest { name: Some("Bob Builder".into()), about: None, avatar_path: None, avatar_data_base64: None }).await
        })
        .await
        .unwrap();
    alice.wait_event("bob renamed himself", |n, v| n == "contact_updated" && v["profile_name"].as_str() == Some("Bob Builder")).await;
    let uname = format!("bob{}", &b_acc.number[..6]);
    let u2 = uname.clone();
    let set = bob.core.run(move |i| async move { i.set_username(Some(format!("@{}", u2.to_uppercase()))).await }).await.unwrap();
    assert_eq!(set.as_deref(), Some(uname.as_str()));
    let u3 = uname.clone();
    let found = alice.core.run(move |i| async move { i.find_account(&u3).await }).await.unwrap().expect("found by username");
    assert_eq!(found.account_id, b_acc.account_id);
    let n = format!("{} {}", &c_acc.number[..8], &c_acc.number[8..]);
    let found = alice.core.run(move |i| async move { i.find_account(&n).await }).await.unwrap().expect("found by number");
    assert_eq!(found.account_id, c_acc.account_id);
    assert!(alice.core.run(|i| async move { i.find_account("nobody_here_123").await }).await.unwrap().is_none());
    let u4 = uname.clone();
    let taken = carol.core.run(move |i| async move { i.set_username(Some(u4)).await }).await;
    assert!(taken.is_err(), "username must be unique");

    // --- attachment -------------------------------------------------------------------
    let mut png = Vec::new();
    image::RgbImage::from_fn(64, 48, |x, y| image::Rgb([x as u8 * 4, y as u8 * 5, 128]))
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    let req: SendMessageRequest = serde_json::from_value(serde_json::json!({
        "conversation_id": bob_conv, "text": "a picture",
        "attachments": [{ "data_base64": base64_encode(&png), "file_name": "pic.png", "mime": "image/png", "kind": "image" }]
    }))
    .unwrap();
    bob.core.run(move |i| async move { i.send_message(req).await }).await.unwrap();
    let pic = alice.wait_message("a picture").await;
    let pic_id = pic["id"].as_str().unwrap().to_string();
    let pid = pic_id.clone();
    let done = alice
        .wait_event("image download", move |n, v| {
            n == "message" && v["message"]["id"].as_str() == Some(pid.as_str()) && v["message"]["attachments"][0]["download_state"] == "done"
        })
        .await;
    let path = done["message"]["attachments"][0]["local_path"].as_str().unwrap().to_string();
    assert_encrypted_file(&path, &png);
    let back = alice.core.run(move |i| async move { i.read_media_file(std::path::Path::new(&path)).await }).await.unwrap();
    assert_eq!(back, png);
    assert_eq!(done["message"]["attachments"][0]["width"], 64);

    // --- contact cards -------------------------------------------------------------------
    let req: SendMessageRequest = serde_json::from_value(serde_json::json!({
        "conversation_id": conv.id, "contact_ids": [a_acc.account_id, b_acc.account_id]
    }))
    .unwrap();
    let shared = alice.core.run(move |i| async move { i.send_message(req).await }).await.unwrap();
    assert_eq!(shared.kind, "contact");
    let sid = shared.id.clone();
    let got = bob.wait_event("contact cards", move |n, v| n == "message" && v["message"]["id"].as_str() == Some(sid.as_str())).await;
    assert_eq!(got["message"]["kind"], "contact");
    assert_eq!(got["message"]["contacts"][0]["number"], a_acc.number.as_str());
    assert_eq!(got["message"]["contacts"][0]["name"], "Alice");
    assert_eq!(got["message"]["contacts"][1]["account_id"], b_acc.account_id.as_str());
    let unknown: SendMessageRequest =
        serde_json::from_value(serde_json::json!({ "conversation_id": conv.id, "contact_ids": ["no-such-account"] })).unwrap();
    assert!(alice.core.run(move |i| async move { i.send_message(unknown).await }).await.is_err());

    // --- voice note (no text, inline bytes, like the UI recorder sends it) ----------------
    let voice: Vec<u8> = (0..20_000u32).map(|i| (i * 7 % 251) as u8).collect();
    let waveform: Vec<f64> = (0..48).map(|i| 0.05 + (i as f64) / 60.0).collect();
    let req: SendMessageRequest = serde_json::from_value(serde_json::json!({
        "conversation_id": bob_conv,
        "attachments": [{ "data_base64": base64_encode(&voice), "file_name": "voice-1.webm", "mime": "audio/webm",
            "kind": "voice", "duration_ms": 3200, "waveform": waveform }]
    }))
    .unwrap();
    let sent_voice = bob.core.run(move |i| async move { i.send_message(req).await }).await.unwrap();
    let got = alice
        .wait_event("voice note", move |n, v| {
            n == "message" && v["message"]["id"].as_str() == Some(sent_voice.id.as_str()) && v["message"]["attachments"][0]["download_state"] == "done"
        })
        .await;
    let path = got["message"]["attachments"][0]["local_path"].as_str().unwrap().to_string();
    assert_encrypted_file(&path, &voice);
    let back = alice.core.run(move |i| async move { i.read_media_file(std::path::Path::new(&path)).await }).await.unwrap();
    assert_eq!(back, voice);
    assert_eq!(got["message"]["attachments"][0]["duration_ms"], 3200);

    // --- second device for Alice (history via vault, sync of sent messages) -------------
    tokio::time::sleep(Duration::from_secs(2)).await; // let the vault flush
    let alice2 = Client::new("alice2");
    let (u, anum) = (url.clone(), a_acc.number.clone());
    let a2 = alice2
        .core
        .run(move |i| async move { i.login(&u, &anum, "correct horse battery", "second", None).await })
        .await
        .unwrap();
    assert_eq!(a2.account_id, a_acc.account_id);
    assert_ne!(a2.device_id, a_acc.device_id);
    alice2.wait_online().await;
    alice2.wait_event("history restored", |n, v| n == "history_sync" && v["done"] == true).await;
    assert!(alice2.saw_message("Hi Alice") || alice2.saw_message("a picture"), "history not restored");

    send_text(&bob, &bob_conv, "to both devices").await;
    alice.wait_message("to both devices").await;
    alice2.wait_message("to both devices").await;
    send_text(&alice, &conv.id, "sent from device 1").await;
    bob.wait_message("sent from device 1").await;
    alice2.wait_message("sent from device 1").await;

    // --- voice call signaling ----------------------------------------------------------
    fn signal(to: &str, kind: &str, data: Value, target: Option<u32>, own: bool) -> chat_core::engine::CallSignalRequest {
        serde_json::from_value(serde_json::json!({
            "account_id": to, "call_id": "call-e2e-1", "kind": kind, "data": data,
            "target_device": target, "to_own_devices": own
        }))
        .unwrap()
    }
    let call_kind = |kind: &'static str| move |n: &str, v: &Value| n == "call_signal" && v["call_id"] == "call-e2e-1" && v["kind"] == kind;
    // Only descriptions our clients produce reach the UI (audio/video, one DTLS fingerprint).
    fn sdp(setup: &str) -> String {
        format!(
            "v=0\r\no=- 1 2 IN IP4 127.0.0.1\r\ns=-\r\nt=0 0\r\nm=audio 9 UDP/TLS/RTP/SAVPF 111\r\nc=IN IP4 0.0.0.0\r\n\
             a=fingerprint:sha-256 4A:AD:B9:B1:3F:82:18:3B:54:02:12:DF:3E:5D:49:6B:19:E5:7C:AB:3D:B4:0A:3E:4E:A9:66:9F:12:E5:8A:01\r\n\
             a=setup:{setup}\r\na=mid:0\r\na=rtpmap:111 opus/48000/2\r\n"
        )
    }
    let (offer_sdp, answer_sdp) = (sdp("actpass"), sdp("active"));
    // A data channel offer is dropped by the core before any UI sees it.
    let bad = format!("{offer_sdp}m=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\na=sctp-port:5000\r\n");
    let req = signal(&a_acc.account_id, "offer", serde_json::json!({ "sdp": bad }), None, false);
    bob.core.run(move |i| async move { i.call_signal(req).await }).await.unwrap();
    // Bob calls Alice: both of her devices ring.
    let req = signal(&a_acc.account_id, "offer", serde_json::json!({ "sdp": offer_sdp }), None, false);
    bob.core.run(move |i| async move { i.call_signal(req).await }).await.unwrap();
    let offer = alice.wait_event("call offer", call_kind("offer")).await;
    alice2.wait_event("call offer on device 2", call_kind("offer")).await;
    assert_eq!(offer["account_id"], b_acc.account_id.as_str());
    assert_eq!(offer["data"]["sdp"], offer_sdp.as_str());
    let offers = alice.events.0.lock().unwrap().iter().filter(|(n, v)| call_kind("offer")(n, v)).count();
    assert_eq!(offers, 1, "the data channel offer must be dropped");
    assert_eq!(offer["conversation_id"], conv.id.as_str());
    let bob_device = offer["device_id"].as_u64().unwrap() as u32;
    // Alice answers on device 1, to Bob's calling device, and stops the ringing elsewhere.
    let req = signal(&b_acc.account_id, "answer", serde_json::json!({ "sdp": answer_sdp }), Some(bob_device), false);
    alice.core.run(move |i| async move { i.call_signal(req).await }).await.unwrap();
    let answer = bob.wait_event("call answer", call_kind("answer")).await;
    assert_eq!(answer["device_id"], a_acc.device_id);
    let req = signal("", "hangup", serde_json::json!({ "reason": "answered_elsewhere" }), None, true);
    alice.core.run(move |i| async move { i.call_signal(req).await }).await.unwrap();
    let elsewhere = alice2.wait_event("answered elsewhere", call_kind("hangup")).await;
    assert_eq!(elsewhere["from_me"], true);
    // ICE for device 2 only.
    let req = signal(&a_acc.account_id, "ice", serde_json::json!({ "candidates": [{ "candidate": "candidate:1 1 udp 2122260223 192.0.2.1 54321 typ host", "sdpMid": "0", "sdpMLineIndex": 0 }] }), Some(a2.device_id), false);
    bob.core.run(move |i| async move { i.call_signal(req).await }).await.unwrap();
    alice2.wait_event("targeted ICE", call_kind("ice")).await;
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert!(!alice.events.0.lock().unwrap().iter().any(|(n, v)| call_kind("ice")(n, v)), "ICE for another device must be dropped");
    // ICE restart of the call in progress: a targeted "reoffer" (never a broadcast), answered
    // with a plain "answer". An invalid description is dropped like an offer's.
    let req = signal(&a_acc.account_id, "reoffer", serde_json::json!({ "sdp": offer_sdp }), None, false);
    assert!(bob.core.run(move |i| async move { i.call_signal(req).await }).await.is_err(), "untargeted restart refused");
    let req = signal(&a_acc.account_id, "reoffer", serde_json::json!({ "sdp": bad }), Some(a_acc.device_id), false);
    bob.core.run(move |i| async move { i.call_signal(req).await }).await.unwrap();
    let req = signal(&a_acc.account_id, "reoffer", serde_json::json!({ "sdp": offer_sdp }), Some(a_acc.device_id), false);
    bob.core.run(move |i| async move { i.call_signal(req).await }).await.unwrap();
    let reoffer = alice.wait_event("call restart", call_kind("reoffer")).await;
    assert_eq!(reoffer["data"]["sdp"], offer_sdp.as_str());
    assert_eq!(reoffer["device_id"], bob_device);
    let reoffers = alice.events.0.lock().unwrap().iter().filter(|(n, v)| call_kind("reoffer")(n, v)).count();
    assert_eq!(reoffers, 1, "the invalid restart must be dropped");
    assert!(!alice2.events.0.lock().unwrap().iter().any(|(n, v)| call_kind("reoffer")(n, v)), "restart for another device");
    let restart_answer = sdp("passive");
    let req = signal(&b_acc.account_id, "answer", serde_json::json!({ "sdp": restart_answer }), Some(bob_device), false);
    alice.core.run(move |i| async move { i.call_signal(req).await }).await.unwrap();
    bob.wait_event("restart answer", |n, v| call_kind("answer")(n, v) && v["data"]["sdp"] == restart_answer.as_str()).await;
    // History entry, once.
    for _ in 0..2 {
        let rec: chat_core::engine::RecordCallRequest = serde_json::from_value(serde_json::json!({
            "conversation_id": conv.id, "call_id": "call-e2e-1", "caller_account_id": b_acc.account_id,
            "status": "answered", "duration_ms": 65_000, "started_at": 1_700_000_000_000i64
        }))
        .unwrap();
        alice.core.run(move |i| async move { i.record_call(rec) }).await.unwrap();
    }
    let cid = conv.id.clone();
    let msgs = alice.core.run(move |i| async move { i.get_messages(&cid, None, Some(200)) }).await.unwrap();
    let calls: Vec<_> = msgs.iter().filter(|m| m.system.as_ref().is_some_and(|s| s.kind == "call")).collect();
    assert_eq!(calls.len(), 1, "one history entry per call");
    let ice = alice.core.run(|i| async move { i.get_ice_servers().await }).await.unwrap();
    assert!(ice["ice_servers"].is_array());

    // --- device linking by QR code, both directions ----------------------------------
    // 1) The new device shows the code, Alice's first device scans and approves.
    let alice3 = Client::new("alice3");
    let u = url.clone();
    let link = alice3.core.run(move |i| async move { i.link_create(&u, "Laptop QR").await }).await.unwrap();
    let waiter = {
        let core = alice3.core.clone();
        tokio::spawn(async move { core.run(|i| async move { i.link_wait().await }).await })
    };
    let l2 = link.clone();
    let name = alice.core.run(move |i| async move { i.link_inspect(&l2).await }).await.unwrap();
    assert_eq!(name, "Laptop QR");
    alice.core.run(move |i| async move { i.link_approve(Some(&link)).await }).await.unwrap();
    let linked = waiter.await.unwrap().unwrap();
    assert_eq!(linked.account_id, a_acc.account_id);
    alice3.wait_online().await;
    // 2) Alice's device shows the code, the new device scans it and Alice approves.
    let alice4 = Client::new("alice4");
    let offer = alice.core.run(|i| async move { i.link_offer().await }).await.unwrap();
    alice4.core.run(move |i| async move { i.link_claim(&offer, "Phone QR").await }).await.unwrap();
    let waiter = {
        let core = alice4.core.clone();
        tokio::spawn(async move { core.run(|i| async move { i.link_wait().await }).await })
    };
    let name = alice.core.run(|i| async move { i.link_offer_wait().await }).await.unwrap();
    // Both screens show the same verification code (photographing the QR is not enough).
    let code_existing = alice.core.run(|i| async move { Ok(i.link_code()) }).await.unwrap();
    let code_new = alice4.core.run(|i| async move { Ok(i.link_code()) }).await.unwrap();
    assert!(code_existing.is_some(), "the approving device must show a code");
    assert_eq!(code_existing, code_new, "both devices must show the same code");
    assert_eq!(name, "Phone QR");
    alice.core.run(|i| async move { i.link_approve(None).await }).await.unwrap();
    let linked = waiter.await.unwrap().unwrap();
    assert_eq!(linked.account_id, a_acc.account_id);
    alice4.wait_online().await;
    // Hostile OS: a linked device does not know the password yet, so nothing is on disk.
    let dir4 = alice4._dir.as_ref().unwrap().path().to_path_buf();
    assert!(!dir4.join("chat.db").exists(), "unprotected data must stay in memory");
    alice4.core.run(|i| async move { i.protect_local_data("correct horse battery").await }).await.unwrap();
    let head = std::fs::read(dir4.join("chat.db")).unwrap();
    assert!(!head.starts_with(b"SQLite format 3"), "the database must be encrypted on disk");
    assert!(dir4.join("chat.db.lock").exists());
    assert!(!dir4.join("db.key").exists(), "no key file next to the database");
    send_text(&bob, &bob_conv, "to all four devices").await;
    for c in [&alice, &alice2, &alice3, &alice4] {
        c.wait_message("to all four devices").await;
    }
    assert!(alice.core.run(|i| async move { i.verify_password("correct horse battery").await }).await.unwrap());
    assert!(!alice.core.run(|i| async move { i.verify_password("nope nope nope").await }).await.unwrap());

    // --- group ---------------------------------------------------------------------------
    let cnum = c_acc.number.clone();
    alice.core.run(move |i| async move { i.add_contact_by_number(&cnum, None).await }).await.unwrap();
    let members = vec![b_acc.account_id.clone(), c_acc.account_id.clone()];
    let group = alice
        .core
        .run(move |i| async move {
            i.create_group(CreateGroupRequest {
                name: "Team".into(),
                description: Some("test group".into()),
                member_account_ids: members,
                avatar_path: None,
                settings: None,
            })
            .await
        })
        .await
        .unwrap();
    assert_eq!(group.member_count, Some(3));
    bob.wait_event("group conv", |n, v| n == "conversation_updated" && v["title"].as_str() == Some("Team")).await;
    carol.wait_event("group conv", |n, v| n == "conversation_updated" && v["title"].as_str() == Some("Team")).await;
    send_text(&alice, &group.id, "hello team").await;
    let m = bob.wait_message("hello team").await;
    let bob_group = m["conversation_id"].as_str().unwrap().to_string();
    let m = carol.wait_message("hello team").await;
    let carol_group = m["conversation_id"].as_str().unwrap().to_string();
    alice2.wait_message("hello team").await;
    send_text(&carol, &carol_group, "hi from carol").await;
    alice.wait_message("hi from carol").await;
    bob.wait_message("hi from carol").await;
    let _ = bob_group;

    // Remove Carol: she must not receive further messages.
    let (gid, cid) = (group.id.clone(), c_acc.account_id.clone());
    let info = alice.core.run(move |i| async move { i.remove_group_member(&gid, &cid).await }).await.unwrap();
    assert_eq!(info.members.len(), 2);
    carol.wait_event("left", |n, v| n == "conversation_updated" && v["left"] == true).await;
    send_text(&alice, &group.id, "after removal").await;
    bob.wait_message("after removal").await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(!carol.saw_message("after removal"));

    // --- recovery with the seed ----------------------------------------------------------
    let bob2 = Client::new("bob-recovered");
    let (u, bnum, words) = (url.clone(), b_acc.number.clone(), bob_mnemonic.clone());
    let rec = bob2
        .core
        .run(move |i| async move { i.recover(&u, &bnum, &words, "new password 123", "recovered").await })
        .await
        .unwrap();
    assert_eq!(rec.account_id, b_acc.account_id);
    bob.wait_event("old device logged out", |n, _| n == "logged_out").await;
    bob2.wait_online().await;
    send_text(&alice, &conv.id, "after recovery").await;
    bob2.wait_message("after recovery").await;

    // Wrong password is rejected; the new one works.
    let bob3 = Client::new("bob3");
    let (u, bnum) = (url.clone(), b_acc.number.clone());
    let err = bob3
        .core
        .run(move |i| async move { i.login(&u, &bnum, "correct horse battery", "x", None).await })
        .await
        .unwrap_err();
    assert_eq!(err.code, chat_core::ErrorCode::InvalidPassword);
    let _ = a_acc;
}

fn base64_encode(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}

/// RFC 6238 code for a base32 secret (what an authenticator app computes).
fn totp_code(secret_b32: &str, step: u64) -> String {
    use hmac::{Hmac, Mac};
    let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let (mut bits, mut acc, mut key) = (0u32, 0u64, Vec::new());
    for c in secret_b32.bytes() {
        acc = (acc << 5) | alphabet.iter().position(|&a| a == c).expect("base32") as u64;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            key.push((acc >> bits) as u8);
        }
    }
    let mut mac = Hmac::<sha1::Sha1>::new_from_slice(&key).unwrap();
    mac.update(&step.to_be_bytes());
    let sum = mac.finalize().into_bytes();
    let off = (sum[19] & 0x0f) as usize;
    let v = u32::from_be_bytes(sum[off..off + 4].try_into().unwrap()) & 0x7fff_ffff;
    format!("{:06}", v % 1_000_000)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_step_verification() {
    if server().is_empty() {
        return;
    }
    let url = server();
    let dave = Client::new("dave");
    let u = url.clone();
    let acc = dave.core.run(move |i| async move { i.register(&u, "correct horse battery", "dave phone").await }).await.unwrap().account;
    let setup = dave.core.run(|i| async move { i.totp_setup().await }).await.unwrap();
    let secret = setup["secret"].as_str().unwrap().to_string();
    assert!(setup["uri"].as_str().unwrap().starts_with("otpauth://totp/"));
    let step = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() / 30;
    let code = totp_code(&secret, step);
    dave.core.run(move |i| async move { i.totp_enable(&code).await }).await.unwrap();
    assert!(dave.core.run(|i| async move { i.account_details().await }).await.unwrap().totp_enabled);

    // A new device: without a code the core reports totp_required, then logs in with one.
    let laptop = Client::new("dave-laptop");
    let (u, num) = (url.clone(), acc.number.clone());
    let err = laptop.core.run(move |i| async move { i.login(&u, &num, "correct horse battery", "laptop", None).await }).await.unwrap_err();
    assert_eq!(err.code.as_str(), "totp_required");
    let (u, num) = (url.clone(), acc.number.clone());
    let err = laptop
        .core
        .run(move |i| async move { i.login(&u, &num, "correct horse battery", "laptop", Some("000000")).await })
        .await
        .unwrap_err();
    assert_eq!(err.code.as_str(), "invalid_totp");
    let (u, num, code) = (url.clone(), acc.number.clone(), totp_code(&secret, step + 1));
    let linked = laptop.core.run(move |i| async move { i.login(&u, &num, "correct horse battery", "laptop", Some(&code)).await }).await.unwrap();
    assert_eq!(linked.account_id, acc.account_id);
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

async fn direct_conv(from: &Client, number: &str) -> Conversation {
    let n = number.to_string();
    let contact = from.core.run(move |i| async move { i.add_contact_by_number(&n, None).await }).await.unwrap();
    from.core.run(move |i| async move { i.open_direct_conversation(&contact.account_id).await }).await.unwrap()
}

async fn messages(c: &Client, conv: &str) -> Vec<Message> {
    let id = conv.to_string();
    c.core.run(move |i| async move { i.get_messages(&id, None, Some(200)) }).await.unwrap()
}

async fn conversation_ids(c: &Client) -> Vec<String> {
    c.core.run(|i| async move { i.list_conversations() }).await.unwrap().into_iter().map(|c| c.id).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn delete_conversations() {
    if server().is_empty() {
        return;
    }
    let url = server();
    let (ana, bob, cris) = (Client::new("ana"), Client::new("bob"), Client::new("cris"));
    let acc = register_all(&url, &[&ana, &bob, &cris]).await;
    let ab = direct_conv(&ana, &acc[1].number).await;
    let ac = direct_conv(&ana, &acc[2].number).await;
    send_text(&ana, &ab.id, "ana to bob").await;
    send_text(&ana, &ac.id, "ana to cris").await;
    bob.wait_message("ana to bob").await;
    cris.wait_message("ana to cris").await;
    let ba = bob.wait_event("conv", |n, v| n == "message" && v["message"]["text"] == "ana to bob").await["message"]["conversation_id"]
        .as_str()
        .unwrap()
        .to_string();
    send_text(&bob, &ba, "bob to ana").await;
    ana.wait_message("bob to ana").await;

    // 1) For me only: Ana's copy goes, Bob keeps his.
    let id = ab.id.clone();
    ana.core.run(move |i| async move { i.delete_conversation(&id) }).await.unwrap();
    assert!(!conversation_ids(&ana).await.contains(&ab.id));
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(messages(&bob, &ba).await.len(), 2, "deleting for me must not touch the peer");

    // 2) For both, while Bob is offline: applied when he comes back.
    send_text(&bob, &ba, "bob again").await;
    ana.wait_message("bob again").await;
    let ab = direct_conv(&ana, &acc[1].number).await;
    bob.core.run(|i| async move { i.stop_background(); Ok(()) }).await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    let id = ab.id.clone();
    ana.core.run(move |i| async move { i.delete_conversation_for_everyone(&id) }).await.unwrap();
    assert!(!conversation_ids(&ana).await.contains(&ab.id));
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(!messages(&bob, &ba).await.is_empty(), "offline: nothing changed yet");
    bob.core.run(|i| async move { i.start_background(); Ok(()) }).await.unwrap();
    let bid = ba.clone();
    bob.wait_event("conversation removed", move |n, v| n == "conversation_removed" && v["conversation_id"].as_str() == Some(bid.as_str())).await;
    assert!(messages(&bob, &ba).await.is_empty(), "the peer's copy must be wiped too");

    // 3) Group: "for everyone" removes only Ana's own messages for the members.
    let members = vec![acc[1].account_id.clone(), acc[2].account_id.clone()];
    let req: CreateGroupRequest = serde_json::from_value(serde_json::json!({ "name": "wipe test", "member_account_ids": members })).unwrap();
    let g = ana.core.run(move |i| async move { i.create_group(req).await }).await.unwrap();
    send_text(&ana, &g.id, "group from ana").await;
    let m = cris.wait_message("group from ana").await;
    let cg = m["conversation_id"].as_str().unwrap().to_string();
    send_text(&cris, &cg, "group from cris").await;
    ana.wait_message("group from cris").await;
    // 3a) A member deleting the group chat only clears it: the group stays usable.
    let id = cg.clone();
    cris.core.run(move |i| async move { i.delete_conversation(&id) }).await.unwrap();
    assert!(conversation_ids(&cris).await.contains(&cg), "an active group must stay in the list");
    assert!(messages(&cris, &cg).await.is_empty());
    send_text(&cris, &cg, "cris still here").await;
    ana.wait_message("cris still here").await;
    // 3b) Members cannot clear the group for everyone.
    let id = cg.clone();
    let err = cris.core.run(move |i| async move { i.delete_conversation_for_everyone(&id) }).await.unwrap_err();
    assert_eq!(err.code.as_str(), "forbidden");
    // 3c) The admin clears the whole history for every member; the group stays.
    let bm = bob.wait_message("cris still here").await;
    let bg = bm["conversation_id"].as_str().unwrap().to_string();
    let id = g.id.clone();
    ana.core.run(move |i| async move { i.delete_conversation_for_everyone(&id) }).await.unwrap();
    for (c, conv) in [(&bob, &bg), (&cris, &cg)] {
        let conv2 = conv.clone();
        c.wait_event("history cleared", move |n, v| {
            n == "message" && v["message"]["conversation_id"].as_str() == Some(conv2.as_str()) && v["message"]["system"]["type"] == "group_history_cleared"
        })
        .await;
        let left = messages(c, conv).await;
        assert_eq!(left.len(), 1, "only the notice remains: {:?}", left.iter().map(|m| &m.text).collect::<Vec<_>>());
        assert!(conversation_ids(c).await.contains(conv));
    }
    assert!(conversation_ids(&ana).await.contains(&g.id));
    send_text(&bob, &bg, "after clear").await;
    ana.wait_message("after clear").await;

    // 4) Wipe everything: Ana has nothing left, Cris' direct chat with Ana is gone too.
    let cc = cris.wait_event("conv", |n, v| n == "message" && v["message"]["text"] == "ana to cris").await["message"]["conversation_id"]
        .as_str()
        .unwrap()
        .to_string();
    // Destructive actions are confirmed with the account password; an unlocked device cannot
    // reset its data without it either (only the lock screen's "forgot password" path can).
    assert!(ana.core.run(|i| async move { i.require_password("not the password").await }).await.is_err());
    ana.core.run(|i| async move { i.require_password("correct horse battery").await }).await.unwrap();
    assert!(ana.core.run(|i| async move { i.reset_local_data_checked(None).await }).await.is_err());
    ana.core.run(|i| async move { i.wipe_all_conversations() }).await.unwrap();
    assert!(conversation_ids(&ana).await.is_empty() || messages(&ana, &ac.id).await.is_empty());
    let cc2 = cc.clone();
    cris.wait_event("direct chat wiped", move |n, v| n == "conversation_removed" && v["conversation_id"].as_str() == Some(cc2.as_str())).await;
}

/// Stops a client and starts it again on the same data, like an app restart.
async fn restart(c: Client, dir: &std::path::Path) -> Client {
    c.core.run(|i| async move { i.stop_background(); Ok(()) }).await.unwrap();
    let name = c.name;
    drop(c);
    tokio::time::sleep(Duration::from_millis(800)).await;
    let c = Client::at(name, dir);
    // The local data is locked with the account password.
    if c.core.run(|i| async move { Ok(i.app_state().status) }).await.unwrap() == "locked" {
        c.core.run(|i| async move { i.unlock("correct horse battery").await }).await.unwrap();
    }
    c.wait_online().await;
    c
}

/// Restarts a client without unlocking it.
async fn restart_locked(c: Client, dir: &std::path::Path) -> Client {
    c.core.run(|i| async move { i.stop_background(); Ok(()) }).await.unwrap();
    let name = c.name;
    drop(c);
    tokio::time::sleep(Duration::from_millis(800)).await;
    Client::at(name, dir)
}

async fn status(c: &Client) -> &'static str {
    c.core.run(|i| async move { Ok(i.app_state().status) }).await.unwrap()
}

async fn unlock(c: &Client, password: &str) -> chat_core::error::Result<()> {
    let p = password.to_string();
    c.core.run(move |i| async move { i.unlock(&p).await }).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn local_password_lock() {
    if server().is_empty() {
        return;
    }
    let url = server();
    let dir = tempfile::tempdir().unwrap();
    let ana = Client::at("ana", dir.path());
    let bob = Client::new("bob");
    let acc = register_all(&url, &[&ana, &bob]).await;
    assert!(ana.core.run(|i| async move { Ok(i.app_state().local_protected) }).await.unwrap(), "locked right after registering");
    let ab = direct_conv(&ana, &acc[1].number).await;
    send_text(&ana, &ab.id, "locked away").await;
    bob.wait_message("locked away").await;

    // The device key (what the OS keystore holds) no longer opens the database.
    let ana = restart_locked(ana, dir.path()).await;
    assert!(chat_core::db::Db::open(&dir.path().join("chat.db"), &[7u8; 32]).is_err());
    assert_eq!(status(&ana).await, "locked");
    assert!(ana.core.run(|i| async move { i.list_conversations() }).await.unwrap().is_empty(), "nothing readable while locked");
    let err = unlock(&ana, "wrong password!!").await.unwrap_err();
    assert_eq!(err.code, chat_core::error::ErrorCode::InvalidPassword);
    unlock(&ana, "correct horse battery").await.unwrap();
    assert_eq!(status(&ana).await, "ready");
    ana.wait_online().await;
    assert_eq!(messages(&ana, &ab.id).await.len(), 1, "history intact after unlocking");

    // Changing the password here re-locks with the new one.
    ana.core.run(|i| async move { i.change_password("correct horse battery", "A brand new secret 1").await }).await.unwrap();
    let ana = restart_locked(ana, dir.path()).await;
    assert!(unlock(&ana, "correct horse battery").await.is_err());
    unlock(&ana, "A brand new secret 1").await.unwrap();
    ana.wait_online().await;

    // Changed on another device: noticed here, and the new password takes over the lock.
    let laptop = Client::new("laptop");
    let (u, num) = (url.clone(), acc[0].number.clone());
    laptop.core.run(move |i| async move { i.login(&u, &num, "A brand new secret 1", "laptop", None).await }).await.unwrap();
    laptop.core.run(|i| async move { i.change_password("A brand new secret 1", "Changed on laptop 2").await }).await.unwrap();
    assert!(ana.core.run(|i| async move { i.account_details().await }).await.unwrap().local_password_outdated);
    assert!(ana.core.run(|i| async move { i.protect_local_data("not the password").await }).await.is_err());
    ana.core.run(|i| async move { i.protect_local_data("Changed on laptop 2").await }).await.unwrap();
    assert!(!ana.core.run(|i| async move { i.account_details().await }).await.unwrap().local_password_outdated);
    assert!(ana.core.run(|i| async move { i.verify_password("Changed on laptop 2").await }).await.unwrap());
    let ana = restart_locked(ana, dir.path()).await;
    assert!(unlock(&ana, "A brand new secret 1").await.is_err());
    unlock(&ana, "Changed on laptop 2").await.unwrap();
    assert_eq!(messages(&ana, &ab.id).await.len(), 1);

    // Forgotten password: everything local goes, the account can be recovered afterwards.
    let ana = restart_locked(ana, dir.path()).await;
    ana.core.run(|i| async move { i.reset_local_data() }).await.unwrap();
    assert_eq!(status(&ana).await, "no_account");
    assert!(!dir.path().join("chat.db.lock").exists());
    let (u, num) = (url.clone(), acc[0].number.clone());
    ana.core.run(move |i| async move { i.login(&u, &num, "Changed on laptop 2", "ana again", None).await }).await.unwrap();
    assert!(ana.core.run(|i| async move { Ok(i.app_state().local_protected) }).await.unwrap());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn temporary_mode() {
    if server().is_empty() {
        return;
    }
    let url = server();
    let dir = tempfile::tempdir().unwrap();
    let ana = Client::at("ana", dir.path());
    let bob = Client::new("bob");
    let acc = register_all(&url, &[&ana, &bob]).await;
    let ba = direct_conv(&bob, &acc[0].number).await;
    send_text(&bob, &ba.id, "saved before").await;
    let conv_id = ana.wait_message("saved before").await["conversation_id"].as_str().unwrap().to_string();
    let c = conv_id.clone();
    ana.core.run(move |i| async move { i.accept_conversation(&c).await }).await.unwrap(); // a message request

    // On: the existing history moves into memory and is gone after a restart.
    ana.core.run(|i| async move { i.set_temporary_mode(true) }).await.unwrap();
    assert_eq!(messages(&ana, &conv_id).await.len(), 1, "still visible during the session");
    let mut png = Vec::new();
    image::RgbImage::from_fn(8, 8, |_, _| image::Rgb([200, 10, 10])).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
    let req: SendMessageRequest = serde_json::from_value(serde_json::json!({
        "conversation_id": ba.id, "text": "temporary photo",
        "attachments": [{ "data_base64": base64_encode(&png), "file_name": "p.png", "mime": "image/png", "kind": "image" }]
    }))
    .unwrap();
    bob.core.run(move |i| async move { i.send_message(req).await }).await.unwrap();
    let got = ana
        .wait_event("photo downloaded", |n, v| n == "message" && v["message"]["text"] == "temporary photo" && v["message"]["attachments"][0]["download_state"] == "done")
        .await;
    let photo = got["message"]["attachments"][0]["local_path"].as_str().unwrap().to_string();
    assert!(photo.contains("media-temp"), "{photo}");
    assert_eq!(messages(&ana, &conv_id).await.len(), 2);

    let ana = restart(ana, dir.path()).await;
    assert!(ana.core.run(|i| async move { Ok(i.temporary_mode()) }).await.unwrap());
    assert!(conversation_ids(&ana).await.is_empty(), "no conversation may survive a restart");
    assert!(!std::path::Path::new(&photo).exists(), "media of temporary chats are deleted");

    // Still works normally in the new session.
    send_text(&bob, &ba.id, "new session").await;
    ana.wait_message("new session").await;

    // Off: from now on messages are saved again.
    ana.core.run(|i| async move { i.set_temporary_mode(false) }).await.unwrap();
    send_text(&bob, &ba.id, "saved after").await;
    ana.wait_message("saved after").await;
    let ana = restart(ana, dir.path()).await;
    let convs = ana.core.run(|i| async move { i.list_conversations() }).await.unwrap();
    assert_eq!(convs.len(), 1);
    let texts: Vec<String> = messages(&ana, &convs[0].id).await.into_iter().filter_map(|m| m.text).collect();
    assert!(texts.contains(&"saved after".to_string()) && texts.contains(&"new session".to_string()), "{texts:?}");
}

async fn set_name(c: &Client, name: &str) {
    let n = name.to_string();
    c.core
        .run(move |i| async move { i.set_profile(SetProfileRequest { name: Some(n), about: None, avatar_path: None, avatar_data_base64: None }).await })
        .await
        .unwrap();
}

async fn group_info(c: &Client, conv: &str) -> GroupInfo {
    let id = conv.to_string();
    c.core.run(move |i| async move { i.group_info(&id) }).await.unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn group_rules() {
    if server().is_empty() {
        return;
    }
    let url = server();
    let (ana, bob, cris) = (Client::new("ana"), Client::new("bob"), Client::new("cris"));
    let acc = register_all(&url, &[&ana, &bob, &cris]).await;
    set_name(&bob, "Bob Builder").await;
    let members = vec![acc[1].account_id.clone(), acc[2].account_id.clone()];
    let rules = serde_json::json!({ "disappearing_seconds": 3600, "hide_members": true, "member_names": "first_name" });
    let req = move || -> CreateGroupRequest {
        serde_json::from_value(serde_json::json!({ "name": "rules", "member_account_ids": members, "settings": rules })).unwrap()
    };

    // "first_name" groups need a profile name, starting with the creator.
    let r = req();
    let err = ana.core.run(move |i| async move { i.create_group(r).await }).await.unwrap_err();
    assert_eq!(err.code, chat_core::error::ErrorCode::ProfileNameRequired);
    set_name(&ana, "Ana Maria Pop").await;
    let r = req();
    let g = ana.core.run(move |i| async move { i.create_group(r).await }).await.unwrap();
    assert_eq!(g.disappearing_seconds, 3600, "creator applies the timer");

    // Members get the timer as the group rule, with a notice.
    let bg = bob
        .wait_event("group with timer", |n, v| n == "conversation_updated" && v["title"] == "rules" && v["disappearing_seconds"] == 3600)
        .await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let bid = bg.clone();
    bob.wait_event("timer notice", move |n, v| {
        n == "message" && v["conversation_id"].as_str() == Some(bid.as_str()) && v["message"]["system"]["type"] == "disappearing_changed"
    })
    .await;
    // Ana is a stranger to Bob: the group is an invite until he joins (his profile key, and
    // with it his name, reaches the members only then).
    let b = bg.clone();
    assert!(!bob.core.run(move |i| async move { i.get_conversation(&b) }).await.unwrap().accepted);
    let b = bg.clone();
    bob.core.run(move |i| async move { i.accept_conversation(&b).await }).await.unwrap();

    // Hidden list: members see only the count, admins see everybody.
    let info = group_info(&bob, &bg).await;
    assert!(info.members_hidden);
    assert_eq!((info.members.len(), info.member_count), (1, 3));
    assert!(info.members[0].is_me);
    let admin_view = group_info(&ana, &g.id).await;
    assert!(!admin_view.members_hidden);
    assert_eq!(admin_view.members.len(), 3);
    // First names only, no account numbers (Bob's profile key reaches Ana via the group).
    let mut bob_row = None;
    for _ in 0..100 {
        let info = group_info(&ana, &g.id).await;
        let row = info.members.into_iter().find(|m| m.account_id == acc[1].account_id).unwrap();
        if row.display_name == "Bob" {
            bob_row = Some(row);
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let bob_row = bob_row.expect("Bob's first name never arrived");
    assert_eq!(bob_row.number, "");

    send_text(&ana, &g.id, "rules apply").await;
    let m = bob.wait_message("rules apply").await;
    assert_eq!(m["sender_name"], "Ana");
    // Disappearing in the group: the timer of a received message starts when it is read.
    assert!(m["expires_at"].is_null(), "timer started before reading");
    let (bconv, rid) = (bg.clone(), m["id"].as_str().unwrap().to_string());
    bob.core.run(move |i| async move { i.mark_read(&bconv) }).await.unwrap();
    let read = bob.core.run(move |i| async move { i.get_message(&rid) }).await.unwrap();
    assert!(read.expires_at.is_some_and(|t| t > chat_core::util::now_ms() + 3_500_000), "timer starts at reading");

    // Cris has no profile name: cannot write until setting one.
    let cg = cris.wait_event("cris group", |n, v| n == "conversation_updated" && v["title"] == "rules").await["id"].as_str().unwrap().to_string();
    let (c2, t) = (cg.clone(), "no name".to_string());
    let req: SendMessageRequest = serde_json::from_value(serde_json::json!({ "conversation_id": c2, "text": t })).unwrap();
    let err = cris.core.run(move |i| async move { i.send_message(req).await }).await.unwrap_err();
    assert_eq!(err.code, chat_core::error::ErrorCode::ProfileNameRequired);
    set_name(&cris, "Cristina Ionescu").await;
    send_text(&cris, &cg, "now with a name").await;
    assert_eq!(bob.wait_message("now with a name").await["sender_name"], "Cristina");

    // Only admins change the timer; the change reaches every member.
    let patch = |conv: &str, d: i64| -> ConversationSettingsPatch {
        serde_json::from_value(serde_json::json!({ "conversation_id": conv, "disappearing_seconds": d })).unwrap()
    };
    let p = patch(&bg, 0);
    let err = bob.core.run(move |i| async move { i.set_conversation_settings(p).await }).await.unwrap_err();
    assert_eq!(err.code, chat_core::error::ErrorCode::Forbidden);
    let p = patch(&g.id, 0);
    ana.core.run(move |i| async move { i.set_conversation_settings(p).await }).await.unwrap();
    bob.wait_event("timer off", |n, v| n == "conversation_updated" && v["title"] == "rules" && v["disappearing_seconds"] == 0).await;

    // Showing the list again.
    let upd: UpdateGroupRequest =
        serde_json::from_value(serde_json::json!({ "conversation_id": g.id, "settings": { "hide_members": false } })).unwrap();
    ana.core.run(move |i| async move { i.update_group(upd).await }).await.unwrap();
    bob.wait_event("list visible", |n, v| n == "group_updated" && v["members_hidden"] == false && v["members"].as_array().map(|a| a.len()) == Some(3))
        .await;
}

/// Post-quantum protection on real sessions: PQXDH (session version 4, the Kyber pre-key is
/// mandatory) and the Triple Ratchet (SPQR state present next to the Double Ratchet).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sessions_are_post_quantum() {
    if server().is_empty() {
        return;
    }
    use libsignal_protocol::{DeviceId, ProtocolAddress, SessionStore};
    let url = server();
    let (ana, bob) = (Client::new("ana"), Client::new("bob"));
    let acc = register_all(&url, &[&ana, &bob]).await;
    let conv = direct_conv(&ana, &acc[1].number).await;
    send_text(&ana, &conv.id, "pq hello").await;
    let bc = bob.wait_message("pq hello").await["conversation_id"].as_str().unwrap().to_string();
    send_text(&bob, &bc, "pq reply").await;
    ana.wait_message("pq reply").await;
    for (c, peer) in [(&ana, acc[1].account_id.clone()), (&bob, acc[0].account_id.clone())] {
        let (version, pq_len) = c
            .core
            .run(move |i| async move {
                let store = i.signal_store()?;
                let address = ProtocolAddress::new(peer, DeviceId::new(1).unwrap());
                let record = store.load_session(&address).await?.expect("a session");
                let pq = record.current_pq_state().map(|s| s.len()).unwrap_or(0);
                Ok((record.session_version()?, pq))
            })
            .await
            .unwrap();
        eprintln!("{}: session version {version}, SPQR state {pq_len} bytes", c.name);
        assert_eq!(version, 4, "{}: not a PQXDH session", c.name);
        assert!(pq_len > 0, "{}: no SPQR (Triple Ratchet) state", c.name);
    }
}

fn assert_encrypted_file(path: &str, plain: &[u8]) {
    let data = std::fs::read(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    assert!(chat_core::media_crypto::is_encrypted(&data), "{path} is not encrypted at rest");
    let probe = &plain[..plain.len().min(32)];
    assert!(!data.windows(probe.len()).any(|w| w == probe), "{path} contains plaintext");
}

async fn wait_downloaded(c: &Client, text: &str) -> Value {
    let t = text.to_string();
    c.wait_event(&format!("download of '{text}'"), move |n, v| {
        n == "message"
            && v["message"]["text"].as_str() == Some(t.as_str())
            && v["message"]["attachments"].as_array().is_some_and(|a| a.iter().all(|x| x["download_state"] == "done"))
    })
    .await["message"]
        .clone()
}

/// Media files are encrypted at rest and removed with their messages (expiry, deletion),
/// except while a forwarded copy still uses them; stray files are swept.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn media_at_rest() {
    if server().is_empty() {
        return;
    }
    use base64::Engine as _;
    let b64 = |d: &[u8]| base64::engine::general_purpose::STANDARD.encode(d);
    let url = server();
    let (ana, bob) = (Client::new("ana"), Client::new("bob"));
    let acc = register_all(&url, &[&ana, &bob]).await;
    let conv = direct_conv(&ana, &acc[1].number).await;
    send_text(&ana, &conv.id, "hello").await;
    let bc = bob.wait_message("hello").await["conversation_id"].as_str().unwrap().to_string();

    let mut png = Vec::new();
    image::RgbImage::from_fn(64, 64, |x, y| image::Rgb([x as u8 * 4, y as u8 * 4, 7]))
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    let doc: Vec<u8> = b"top secret document ".iter().cycle().take(200_000).copied().collect();
    let send_files = |text: &str, conv: &str| {
        let req: SendMessageRequest = serde_json::from_value(serde_json::json!({ "conversation_id": conv, "text": text, "attachments": [
            { "data_base64": b64(&png), "file_name": "p.png", "mime": "image/png", "kind": "image" },
            { "data_base64": b64(&doc), "file_name": "d.txt", "mime": "text/plain", "kind": "file" }
        ]}))
        .unwrap();
        ana.core.run(move |i| async move { i.send_message(req).await })
    };
    let download_all = |c: &Client, m: &Value| {
        let mid = m["id"].as_str().unwrap().to_string();
        let ids: Vec<String> = m["attachments"].as_array().unwrap().iter().map(|a| a["id"].as_str().unwrap().to_string()).collect();
        let core = c.core.clone();
        async move {
            for a in ids {
                let m2 = mid.clone();
                core.run(move |i| async move { i.download_attachment(&m2, &a).await }).await.unwrap();
            }
        }
    };

    // 1. Encrypted on both sides, readable through the core.
    let sent = send_files("files", &conv.id).await.unwrap();
    for (a, plain) in sent.attachments.iter().zip([&png, &doc]) {
        assert_encrypted_file(a.local_path.as_deref().unwrap(), plain);
    }
    let got = bob.wait_message("files").await;
    download_all(&bob, &got).await;
    let got = wait_downloaded(&bob, "files").await;
    let bob_paths: Vec<String> = got["attachments"].as_array().unwrap().iter().map(|a| a["local_path"].as_str().unwrap().to_string()).collect();
    for (p, plain) in bob_paths.iter().zip([&png, &doc]) {
        assert_encrypted_file(p, plain);
        let pp = p.clone();
        let back = bob.core.run(move |i| async move { i.read_media_base64(&pp).await }).await.unwrap();
        assert_eq!(base64::engine::general_purpose::STANDARD.decode(back).unwrap(), **plain, "decrypted copy differs");
    }
    // The user's own export is the only plain copy.
    let dest = tempfile::tempdir().unwrap();
    let out = dest.path().join("d.txt").to_string_lossy().to_string();
    let (mid, aid, o2) = (got["id"].as_str().unwrap().to_string(), got["attachments"][1]["id"].as_str().unwrap().to_string(), out.clone());
    bob.core.run(move |i| async move { i.save_attachment(&mid, &aid, &o2).await }).await.unwrap();
    assert_eq!(std::fs::read(&out).unwrap(), doc);

    // 2. Avatars too.
    let mut avatar = Vec::new();
    image::RgbImage::from_fn(80, 80, |_, _| image::Rgb([200, 30, 30]))
        .write_to(&mut std::io::Cursor::new(&mut avatar), image::ImageFormat::Png)
        .unwrap();
    let av = b64(&avatar);
    let p = ana
        .core
        .run(move |i| async move { i.set_profile(SetProfileRequest { name: Some("Ana".into()), about: None, avatar_path: None, avatar_data_base64: Some(av) }).await })
        .await
        .unwrap();
    assert!(chat_core::media_crypto::is_encrypted(&std::fs::read(p.avatar_path.unwrap()).unwrap()));

    // 3. Disappearing messages take their files with them, on both sides.
    let patch: ConversationSettingsPatch = serde_json::from_value(serde_json::json!({ "conversation_id": conv.id, "disappearing_seconds": 4 })).unwrap();
    ana.core.run(move |i| async move { i.set_conversation_settings(patch).await }).await.unwrap();
    let sent = send_files("expiring", &conv.id).await.unwrap();
    let got = bob.wait_message("expiring").await;
    download_all(&bob, &got).await;
    let got = wait_downloaded(&bob, "expiring").await;
    // Received disappearing messages start their timer when read.
    let b_read = bc.clone();
    bob.core.run(move |i| async move { i.mark_read(&b_read) }).await.unwrap();
    let mut files: Vec<String> = sent.attachments.iter().map(|a| a.local_path.clone().unwrap()).collect();
    files.extend(got["attachments"].as_array().unwrap().iter().map(|a| a["local_path"].as_str().unwrap().to_string()));
    assert!(files.iter().all(|f| std::path::Path::new(f).exists()));
    let mut gone = false;
    for _ in 0..40 {
        tokio::time::sleep(Duration::from_millis(500)).await;
        if files.iter().all(|f| !std::path::Path::new(f).exists()) {
            gone = true;
            break;
        }
    }
    assert!(gone, "expired message files still on disk: {files:?}");
    // The timer applies to the messages already in the chat too: the earlier files went as well.
    assert!(bob_paths.iter().all(|p| !std::path::Path::new(p).exists()), "earlier messages kept their files");
    let patch: ConversationSettingsPatch = serde_json::from_value(serde_json::json!({ "conversation_id": conv.id, "disappearing_seconds": 0 })).unwrap();
    ana.core.run(move |i| async move { i.set_conversation_settings(patch).await }).await.unwrap();
    // Files of a message sent with the timer off, for the sweep checks below.
    send_files("kept", &conv.id).await.unwrap();
    let got = bob.wait_message("kept").await;
    download_all(&bob, &got).await;
    let got = wait_downloaded(&bob, "kept").await;
    let bob_paths: Vec<String> = got["attachments"].as_array().unwrap().iter().map(|a| a["local_path"].as_str().unwrap().to_string()).collect();

    // 4. A forwarded copy keeps the shared file alive until it goes too.
    let orig = send_files("to forward", &conv.id).await.unwrap();
    let shared = orig.attachments[1].local_path.clone().unwrap();
    let (ids, convs) = (vec![orig.id.clone()], vec![conv.id.clone()]);
    ana.core.run(move |i| async move { i.forward_messages(&ids, &convs) }).await.unwrap();
    let oid = orig.id.clone();
    ana.core.run(move |i| async move { i.delete_message_local(&oid) }).await.unwrap();
    assert!(std::path::Path::new(&shared).exists(), "deleting the original removed the forwarded copy's file");
    let copy = messages(&ana, &conv.id).await.into_iter().find(|m| m.forwarded).expect("forwarded copy");
    ana.core.run(move |i| async move { i.delete_message_local(&copy.id) }).await.unwrap();
    assert!(!std::path::Path::new(&shared).exists(), "last user gone, file must go");

    // 5. Stray files are swept, files in use are not; old plaintext files get encrypted.
    let media_dir = std::path::Path::new(&bob_paths[0]).parent().unwrap().to_path_buf();
    let stray = media_dir.join("stray.bin");
    std::fs::write(&stray, b"leftover").unwrap();
    let old = std::time::SystemTime::now() - Duration::from_secs(3600);
    std::fs::File::options().write(true).open(&stray).unwrap().set_modified(old).unwrap();
    let legacy = media_dir.join("legacy.txt");
    std::fs::write(&legacy, &doc).unwrap();
    let n = bob.core.run(|i| async move { i.encrypt_legacy_media().await }).await.unwrap();
    assert!(n >= 2);
    assert_encrypted_file(legacy.to_str().unwrap(), &doc);
    std::fs::File::options().write(true).open(&stray).unwrap().set_modified(old).unwrap();
    bob.core.run(|i| async move { i.sweep_orphan_media() }).await.unwrap();
    assert!(!stray.exists(), "unreferenced file not swept");
    assert!(bob_paths.iter().all(|p| std::path::Path::new(p).exists()), "a file in use was swept");

    // 6. Deleting the conversation deletes its files.
    let b2 = bc.clone();
    bob.core.run(move |i| async move { i.delete_conversation(&b2) }).await.unwrap();
    assert!(bob_paths.iter().all(|p| !std::path::Path::new(p).exists()), "files of a deleted conversation remain");
}

/// New passwords must meet the rules; a verified phone number makes the account findable by
/// search and by phone-contact import, unless hidden.
/// The SMS code is read from the dev server log (CHATAPP_E2E_SERVER_LOG, log SMS provider).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn password_rules_and_phone_search() {
    if server().is_empty() {
        return;
    }
    let url = server();
    // Weak passwords are refused when registering.
    let c = Client::new("weak");
    for weak in ["short", "alllowercaseletters", "Password123!"] {
        let (u, w) = (url.clone(), weak.to_string());
        let err = c.core.run(move |i| async move { i.register(&u, &w, "x").await }).await.unwrap_err();
        assert_eq!(err.code, chat_core::error::ErrorCode::WeakPassword, "{weak}");
    }
    let (ana, bob) = (Client::new("ana"), Client::new("bob"));
    let acc = register_all(&url, &[&ana, &bob]).await;
    // ...and when changing it.
    let err = ana.core.run(|i| async move { i.change_password("correct horse battery", "weakpass").await }).await.unwrap_err();
    assert_eq!(err.code, chat_core::error::ErrorCode::WeakPassword);
    assert!(!ana.core.run(|i| async move { i.account_details().await }).await.unwrap().password_weak);

    let Ok(log) = std::env::var("CHATAPP_E2E_SERVER_LOG") else {
        eprintln!("CHATAPP_E2E_SERVER_LOG not set, skipping the phone part");
        return;
    };
    let phone = format!("+4072{:07}", rand::random::<u32>() % 10_000_000);
    let p = phone.clone();
    ana.core.run(move |i| async move { i.request_phone_code(&p).await }).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let text = std::fs::read_to_string(&log).unwrap();
    let line = text.lines().rev().find(|l| l.contains(&phone) && l.contains("verification code")).expect("sms in log");
    let code: String = line.rsplit("code is ").next().unwrap().chars().take(6).collect();
    let p = phone.clone();
    ana.core.run(move |i| async move { i.verify_phone(&p, &code).await }).await.unwrap();

    // Found by the international number, with spaces, by Bob.
    let spaced = format!("{} {} {}", &phone[..3], &phone[3..7], &phone[7..]);
    let found = bob.core.run(move |i| async move { i.find_account(&spaced).await }).await.unwrap().expect("found by phone");
    assert_eq!(found.account_id, acc[0].account_id);
    // And through an imported address book.
    let dir = tempfile::tempdir().unwrap();
    let vcf = dir.path().join("contacts.vcf");
    std::fs::write(&vcf, format!("BEGIN:VCARD\nVERSION:3.0\nFN:Ana Phone\nTEL:{phone}\nEND:VCARD\n")).unwrap();
    let path = vcf.to_string_lossy().to_string();
    let res = bob.core.run(move |i| async move { i.import_contacts(&path).await }).await.unwrap();
    assert_eq!(res.matched, 1, "{res:?}");

    // Hidden: neither search nor import finds it.
    ana.core.run(|i| async move { i.set_discoverable(false).await }).await.unwrap();
    let p = phone.clone();
    assert!(bob.core.run(move |i| async move { i.find_account(&p).await }).await.unwrap().is_none());
}

/// @mentions travel with the message, survive edits, mark the message for the mentioned
/// member only, and count as unread mentions.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mentions() {
    if server().is_empty() {
        return;
    }
    let url = server();
    let (ana, bob, cris) = (Client::new("ana"), Client::new("bob"), Client::new("cris"));
    let acc = register_all(&url, &[&ana, &bob, &cris]).await;
    let members = vec![acc[1].account_id.clone(), acc[2].account_id.clone()];
    let req: CreateGroupRequest = serde_json::from_value(serde_json::json!({ "name": "mentions", "member_account_ids": members })).unwrap();
    let g = ana.core.run(move |i| async move { i.create_group(req).await }).await.unwrap();
    // "Salut @Bob 😀 ce faci?" — the emoji before nothing, but UTF-16 offsets must hold:
    // "Salut " is 6 units, "@Bob" 4.
    let text = "Salut @Bob 😀 ce faci?";
    let req: SendMessageRequest = serde_json::from_value(serde_json::json!({
        "conversation_id": g.id, "text": text,
        "mentions": [
            { "account_id": acc[1].account_id, "start": 6, "length": 4 },
            { "account_id": acc[2].account_id, "start": 8, "length": 4 },   // overlaps: dropped
            { "account_id": acc[2].account_id, "start": 100, "length": 4 }, // outside: dropped
            { "account_id": "not-a-uuid", "start": 0, "length": 5 }          // invalid: dropped
        ]
    }))
    .unwrap();
    let sent = ana.core.run(move |i| async move { i.send_message(req).await }).await.unwrap();
    assert_eq!(sent.mentions.len(), 1);
    assert_eq!(sent.text.as_deref(), Some(text));
    let at_bob = bob.wait_message(text).await;
    assert_eq!(at_bob["mentions_me"], true);
    assert_eq!(at_bob["mentions"][0]["account_id"], acc[1].account_id.as_str());
    assert_eq!((at_bob["mentions"][0]["start"].as_u64(), at_bob["mentions"][0]["length"].as_u64()), (Some(6), Some(4)));
    let at_cris = cris.wait_message(text).await;
    assert_eq!(at_cris["mentions_me"], false);
    // Unread mention counter for Bob only.
    let bg = at_bob["conversation_id"].as_str().unwrap().to_string();
    let b2 = bg.clone();
    let conv = bob.core.run(move |i| async move { i.get_conversation(&b2) }).await.unwrap();
    assert_eq!(conv.unread_mentions, 1);
    let cg = at_cris["conversation_id"].as_str().unwrap().to_string();
    let conv = cris.core.run(move |i| async move { i.get_conversation(&cg) }).await.unwrap();
    assert_eq!(conv.unread_mentions, 0);
    let b3 = bg.clone();
    bob.core.run(move |i| async move { i.mark_read(&b3) }).await.unwrap();
    let b4 = bg.clone();
    assert_eq!(bob.core.run(move |i| async move { i.get_conversation(&b4) }).await.unwrap().unread_mentions, 0);

    // An edit can move the mention to Cris.
    let edited = "@Cris ai văzut?";
    let mentions: Vec<Mention> = serde_json::from_value(serde_json::json!([{ "account_id": acc[2].account_id, "start": 0, "length": 5 }])).unwrap();
    let id = sent.id.clone();
    ana.core.run(move |i| async move { i.edit_message(&id, edited, &mentions) }).await.unwrap();
    let e = cris.wait_event("edit with mention", |n, v| n == "message" && v["message"]["text"] == "@Cris ai văzut?").await;
    assert_eq!(e["message"]["mentions_me"], true);
    let e = bob.wait_event("edit seen by bob", |n, v| n == "message" && v["message"]["text"] == "@Cris ai văzut?").await;
    assert_eq!(e["message"]["mentions_me"], false);
}

/// Pinned and starred messages: local only, at most 3 pins per chat, they expire and go
/// away with their message.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pins_and_stars() {
    if server().is_empty() {
        return;
    }
    let url = server();
    let (ana, bob) = (Client::new("ana"), Client::new("bob"));
    let acc = register_all(&url, &[&ana, &bob]).await;
    let conv = direct_conv(&ana, &acc[1].number).await;
    let mut ids = Vec::new();
    for n in 0..5 {
        ids.push(send_text(&ana, &conv.id, &format!("pin {n}")).await.id);
    }
    bob.wait_message("pin 4").await;
    let pin = |c: &Client, id: &str, secs: Option<i64>| {
        let id = id.to_string();
        let core = c.core.clone();
        async move { core.run(move |i| async move { i.pin_message(&id, secs) }).await }
    };
    // Unsupported duration.
    assert!(pin(&ana, &ids[0], Some(60)).await.is_err());
    for id in &ids[..4] {
        let m = pin(&ana, id, Some(86_400)).await.unwrap();
        assert!(m.pinned_until.is_some());
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let c2 = conv.id.clone();
    let pinned = ana.core.run(move |i| async move { i.pinned_messages(&c2) }).await.unwrap();
    let texts: Vec<_> = pinned.iter().filter_map(|m| m.text.clone()).collect();
    assert_eq!(texts, ["pin 3", "pin 2", "pin 1"], "max 3, newest first; the oldest was unpinned");
    // Local only: Bob sees nothing.
    let bc = bob.wait_event("conv", |n, v| n == "message" && v["message"]["text"] == "pin 0").await["message"]["conversation_id"]
        .as_str()
        .unwrap()
        .to_string();
    let b2 = bc.clone();
    assert!(bob.core.run(move |i| async move { i.pinned_messages(&b2) }).await.unwrap().is_empty());
    assert!(messages(&bob, &bc).await.iter().all(|m| m.pinned_until.is_none() && !m.starred));
    // Unpin.
    pin(&ana, &ids[2], None).await.unwrap();
    let c2 = conv.id.clone();
    assert_eq!(ana.core.run(move |i| async move { i.pinned_messages(&c2) }).await.unwrap().len(), 2);

    // Stars.
    for id in [&ids[0], &ids[1]] {
        let id = id.clone();
        let m = ana.core.run(move |i| async move { i.star_message(&id, true) }).await.unwrap();
        assert!(m.starred);
    }
    let starred = ana.core.run(|i| async move { i.starred_messages() }).await.unwrap();
    assert_eq!(starred.iter().filter_map(|m| m.text.clone()).collect::<Vec<_>>(), ["pin 1", "pin 0"]);

    // Deleting a message drops its pin and star.
    let id = ids[1].clone();
    ana.core.run(move |i| async move { i.delete_message(&id, false) }).await.unwrap();
    let starred = ana.core.run(|i| async move { i.starred_messages() }).await.unwrap();
    assert_eq!(starred.len(), 1);
    let c2 = conv.id.clone();
    let pinned = ana.core.run(move |i| async move { i.pinned_messages(&c2) }).await.unwrap();
    assert_eq!(pinned.iter().filter_map(|m| m.text.clone()).collect::<Vec<_>>(), ["pin 3"]);
    let left: i64 = ana
        .core
        .run(|i| async move { Ok(i.db.borrow().conn.query_row("SELECT COUNT(*) FROM message_marks", [], |r| r.get(0)).unwrap()) })
        .await
        .unwrap();
    assert_eq!(left, 2, "pin 3 and the star of pin 0 remain");

    // Expiry: pins and stars end on their own (shortened here).
    ana.events.0.lock().unwrap().clear();
    ana.core
        .run(|i| async move {
            let soon = chat_core::util::now_ms() + 1500;
            i.db.borrow().conn.execute("UPDATE message_marks SET pinned_until = MIN(pinned_until, ?1), starred_until = MIN(starred_until, ?1)", [soon]).unwrap();
            i.expiry_notify.notify_one();
            Ok(())
        })
        .await
        .unwrap();
    let c2 = conv.id.clone();
    ana.wait_event("pins expired", move |n, v| n == "pins_changed" && v["conversation_id"].as_str() == Some(c2.as_str())).await;
    let c2 = conv.id.clone();
    assert!(ana.core.run(move |i| async move { i.pinned_messages(&c2) }).await.unwrap().is_empty());
    assert!(ana.core.run(|i| async move { i.starred_messages() }).await.unwrap().is_empty());
    assert!(messages(&ana, &conv.id).await.iter().all(|m| m.pinned_until.is_none() && !m.starred));
}

/// Group invite links: joining directly, with approval, rejected requests, the link replaced
/// when a member is removed, and admin-only management.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn group_invite_links() {
    use chat_core::engine::invites::SetInviteRequest;
    if server().is_empty() {
        return;
    }
    let url = server();
    let (ana, bob, cris, dan, erin) = (Client::new("ana"), Client::new("bob"), Client::new("cris"), Client::new("dan"), Client::new("erin"));
    let acc = register_all(&url, &[&ana, &bob, &cris, &dan, &erin]).await;
    for (c, n) in [(&dan, "Dan Rusu"), (&erin, "Erin")] {
        let n = n.to_string();
        c.core
            .run(move |i| async move { i.set_profile(SetProfileRequest { name: Some(n), about: None, avatar_path: None, avatar_data_base64: None }).await })
            .await
            .unwrap();
    }
    let req: CreateGroupRequest =
        serde_json::from_value(serde_json::json!({ "name": "Invite club", "description": "via link", "member_account_ids": [acc[1].account_id] })).unwrap();
    let g = ana.core.run(move |i| async move { i.create_group(req).await }).await.unwrap();
    let set = |c: &Client, conv: &str, enabled: bool, approval: Option<bool>, reset: bool| {
        let req = SetInviteRequest { conversation_id: conv.to_string(), enabled, approval, reset };
        let core = c.core.clone();
        async move { core.run(move |i| async move { i.set_group_invite(req).await }).await }
    };
    let info = set(&ana, &g.id, true, Some(false), false).await.unwrap();
    let link = info.invite_link.clone().expect("admin sees the link");
    assert!(link.starts_with("chatapp://join/"));

    // A member (not admin) cannot manage it, nor see it.
    let bm = bob.wait_event("bob in group", |n, v| n == "conversation_updated" && v["title"] == "Invite club").await;
    let bg = bm["id"].as_str().unwrap().to_string();
    let b2 = bg.clone();
    let err = set(&bob, &bg, true, None, true).await.unwrap_err();
    assert_eq!(err.code.as_str(), "forbidden");
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(bob.core.run(move |i| async move { i.group_info(&b2) }).await.unwrap().invite_link.is_none());

    // Preview and join without approval.
    let l = link.clone();
    let p = cris.core.run(move |i| async move { i.preview_group_invite(&l).await }).await.unwrap();
    assert_eq!((p.name.as_str(), p.description.as_str(), p.member_count, p.approval, p.is_member), ("Invite club", "via link", 2, false, false));
    let l = link.clone();
    let j = cris.core.run(move |i| async move { i.join_group_by_link(&l).await }).await.unwrap();
    assert_eq!(j.status, "joined");
    let cg = j.conversation_id.unwrap();
    let c2 = cg.clone();
    let ci = cris.core.run(move |i| async move { i.group_info(&c2) }).await.unwrap();
    assert_eq!(ci.name, "Invite club", "metadata readable with the key from the link");
    assert_eq!(ci.member_count, 3);
    ana.wait_event("joined notice", |n, v| n == "message" && v["message"]["system"]["type"] == "group_member_joined_link").await;
    send_text(&cris, &cg, "hello from cris").await;
    bob.wait_message("hello from cris").await;
    send_text(&bob, &bg, "welcome cris").await;
    cris.wait_message("welcome cris").await;

    // Approval: same link, the request carries the requester's name.
    set(&ana, &g.id, true, Some(true), false).await.unwrap();
    let l = link.clone();
    let j = dan.core.run(move |i| async move { i.join_group_by_link(&l).await }).await.unwrap();
    assert_eq!((j.status.as_str(), j.conversation_id.is_none()), ("requested", true));
    let gid = ana.wait_event("request count", |n, v| n == "group_updated" && v["join_requests"] == 1).await["group_id"].as_str().unwrap().to_string();
    let a2 = g.id.clone();
    let reqs = ana.core.run(move |i| async move { i.group_join_requests(&a2).await }).await.unwrap();
    assert_eq!(reqs.len(), 1);
    assert_eq!((reqs[0].account_id.as_str(), reqs[0].name.as_str()), (acc[3].account_id.as_str(), "Dan Rusu"));
    let l = link.clone();
    assert!(dan.core.run(move |i| async move { i.preview_group_invite(&l).await }).await.unwrap().requested);
    let (a2, d) = (g.id.clone(), acc[3].account_id.clone());
    ana.core.run(move |i| async move { i.approve_join_request(&a2, &d).await }).await.unwrap();
    let dm = dan.wait_event("dan in group", |n, v| n == "group_updated" && v["name"] == "Invite club" && v["member_count"] == 4).await;
    let dg = dm["conversation_id"].as_str().unwrap().to_string();
    send_text(&ana, &g.id, "hi dan").await;
    dan.wait_message("hi dan").await;
    send_text(&dan, &dg, "thanks").await;
    cris.wait_message("thanks").await;

    // Rejected request.
    let l = link.clone();
    erin.core.run(move |i| async move { i.join_group_by_link(&l).await }).await.unwrap();
    let a2 = g.id.clone();
    let reqs = ana.core.run(move |i| async move { i.group_join_requests(&a2).await }).await.unwrap();
    assert_eq!(reqs[0].name, "Erin");
    let (a2, e) = (g.id.clone(), acc[4].account_id.clone());
    let info = ana.core.run(move |i| async move { i.reject_join_request(&a2, &e).await }).await.unwrap();
    assert_eq!((info.join_requests, info.member_count), (0, 4));
    let l = link.clone();
    let p = erin.core.run(move |i| async move { i.preview_group_invite(&l).await }).await.unwrap();
    assert!(!p.is_member && !p.requested);

    // Removing a member replaces the link; the old one stops working.
    let (a2, c) = (g.id.clone(), acc[2].account_id.clone());
    let info = ana.core.run(move |i| async move { i.remove_group_member(&a2, &c).await }).await.unwrap();
    let new_link = info.invite_link.clone().unwrap();
    assert_ne!(new_link, link);
    assert!(info.invite_approval, "approval kept");
    let l = link.clone();
    let err = cris.core.run(move |i| async move { i.preview_group_invite(&l).await }).await.unwrap_err();
    assert_eq!(err.code.as_str(), "not_found");
    let l = new_link.clone();
    let p = erin.core.run(move |i| async move { i.preview_group_invite(&l).await }).await.unwrap();
    assert_eq!(p.name, "Invite club");
    let _ = gid;

    // Rejected and removed accounts are banned: no rejoining by link until an admin lifts it.
    assert!(p.banned, "rejected requester is banned");
    let l = new_link.clone();
    let err = cris.core.run(move |i| async move { i.join_group_by_link(&l).await }).await.unwrap_err();
    assert_eq!(err.code.as_str(), "banned");
    let a2 = g.id.clone();
    let bans = ana.core.run(move |i| async move { i.group_bans(&a2).await }).await.unwrap();
    let mut ids: Vec<String> = bans.iter().map(|b| b.account_id.clone()).collect();
    ids.sort();
    let mut want = vec![acc[2].account_id.clone(), acc[4].account_id.clone()];
    want.sort();
    assert_eq!(ids, want);
    let b2 = bg.clone();
    let err = bob.core.run(move |i| async move { i.group_bans(&b2).await }).await.unwrap_err();
    assert_eq!(err.code.as_str(), "forbidden");
    let (a2, e) = (g.id.clone(), acc[4].account_id.clone());
    ana.core.run(move |i| async move { i.unban_group_member(&a2, &e).await }).await.unwrap();
    let (a2, e) = (g.id.clone(), acc[4].account_id.clone());
    let err = ana.core.run(move |i| async move { i.unban_group_member(&a2, &e).await }).await.unwrap_err();
    assert_eq!(err.code.as_str(), "not_found");
    let l = new_link.clone();
    assert!(!erin.core.run(move |i| async move { i.preview_group_invite(&l).await }).await.unwrap().banned);
    let l = new_link.clone();
    let j = erin.core.run(move |i| async move { i.join_group_by_link(&l).await }).await.unwrap();
    assert_eq!(j.status, "requested", "unbanned: may ask again");
    // Adding a banned account back lifts the ban.
    let (a2, c) = (g.id.clone(), acc[2].account_id.clone());
    ana.core.run(move |i| async move { i.add_group_members(&a2, vec![c]).await }).await.unwrap();
    let a2 = g.id.clone();
    assert!(ana.core.run(move |i| async move { i.group_bans(&a2).await }).await.unwrap().is_empty());

    // Reset, then off.
    let info = set(&ana, &g.id, true, None, true).await.unwrap();
    assert_ne!(info.invite_link.as_deref(), Some(new_link.as_str()));
    let info = set(&ana, &g.id, false, None, false).await.unwrap();
    assert!(info.invite_link.is_none());
    let l = new_link.clone();
    assert!(erin.core.run(move |i| async move { i.preview_group_invite(&l).await }).await.is_err());
}

/// A malicious server adds an account of its own to a group (simulated by writing to the
/// server's database). The members must not hand it their keys: it receives the group
/// ciphertext but cannot read it, and the members see it flagged as unverified.
///
/// Needs `CHATAPP_E2E_SQL`: a shell command that runs SQL from stdin against the server's
/// database, e.g. `docker exec -i chatapp-mysql-1 sh -c 'MYSQL_PWD=... mysql -uchatapp chatapp_rs'`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn server_cannot_inject_group_member() {
    let sql_cmd = std::env::var("CHATAPP_E2E_SQL").unwrap_or_default();
    if server().is_empty() || sql_cmd.is_empty() {
        eprintln!("CHATAPP_E2E_SERVER / CHATAPP_E2E_SQL not set, skipping");
        return;
    }
    let url = server();
    let (ana, bob, ghost) = (Client::new("ana"), Client::new("bob"), Client::new("ghost"));
    let acc = register_all(&url, &[&ana, &bob, &ghost]).await;
    let req: CreateGroupRequest =
        serde_json::from_value(serde_json::json!({ "name": "private", "member_account_ids": [acc[1].account_id] })).unwrap();
    let g = ana.core.run(move |i| async move { i.create_group(req).await }).await.unwrap();
    send_text(&ana, &g.id, "before the ghost").await;
    let bm = bob.wait_message("before the ghost").await;
    let bob_conv = bm["conversation_id"].as_str().unwrap().to_string();
    let conv = g.id.clone();
    let gid = ana.core.run(move |i| async move { i.group_info(&conv) }).await.unwrap().group_id;

    // The server adds its own account to the group, without any admin.
    let sql = format!(
        "INSERT INTO group_members (group_id, account_id, role, joined_at, added_by) VALUES ('{gid}', '{ghost}', 'member', {now}, '{ana}');
         UPDATE chat_groups SET revision = revision + 1 WHERE id = '{gid}';",
        ghost = acc[2].account_id,
        ana = acc[0].account_id,
        now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis()
    );
    let mut child = std::process::Command::new("sh")
        .args(["-c", &sql_cmd])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .expect("run CHATAPP_E2E_SQL");
    use std::io::Write;
    child.stdin.take().unwrap().write_all(sql.as_bytes()).unwrap();
    assert!(child.wait().unwrap().success(), "SQL injection of the ghost failed");
    // The ghost runs a modified client: it accepts whatever keys it is sent.
    #[cfg(feature = "insecure-test-hooks")]
    {
        let gid2 = gid.clone();
        ghost.core.run(move |i| async move { i.insecure_trust_server_group(&gid2).await }).await.unwrap();
    }

    // Both members keep talking (the server now insists the ghost gets the ciphertext).
    send_text(&ana, &g.id, "secret after the ghost").await;
    bob.wait_message("secret after the ghost").await;
    send_text(&bob, &bob_conv, "bob's secret").await;
    ana.wait_message("bob's secret").await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(!ghost.saw_message("secret after the ghost"), "the ghost must not read the group");
    assert!(!ghost.saw_message("bob's secret"), "the ghost must not read the group");

    // The members see the ghost listed by the server but not confirmed by any admin.
    for (c, conv) in [(&ana, g.id.clone()), (&bob, bob_conv.clone())] {
        let cv = conv.clone();
        let info = c.core.run(move |i| async move { i.group_info(&cv) }).await.unwrap();
        let ghost_entry = info.members.iter().find(|m| m.account_id == acc[2].account_id);
        assert!(ghost_entry.is_some_and(|m| !m.verified), "{}: the ghost must be flagged unverified", c.name);
        assert!(info.members.iter().filter(|m| m.account_id != acc[2].account_id).all(|m| m.verified));
        let msgs = messages(c, &conv).await;
        assert!(
            msgs.iter().any(|m| m.system.as_ref().is_some_and(|s| s.kind == "group_member_unverified")),
            "{}: missing the unverified-member notice",
            c.name
        );
    }

    // A real addition by the admin is confirmed end to end and works normally.
    let carol = Client::new("carol");
    let carol_acc = register_all(&url, &[&carol]).await.remove(0);
    let (conv, cid) = (g.id.clone(), carol_acc.account_id.clone());
    ana.core.run(move |i| async move { i.add_group_members(&conv, vec![cid]).await }).await.unwrap();
    send_text(&ana, &g.id, "welcome carol").await;
    let cm = carol.wait_message("welcome carol").await;
    let carol_conv = cm["conversation_id"].as_str().unwrap().to_string();
    let info = carol.core.run(move |i| async move { i.group_info(&carol_conv) }).await.unwrap();
    assert!(info.members.iter().find(|m| m.account_id == acc[0].account_id).is_some_and(|m| m.verified));
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(!ghost.saw_message("welcome carol"));
}

/// zkgroup step 1: the client pins the server's parameters, fetches a week of auth
/// credentials, verifies them, and presents one for a group (the presentation is checked
/// against the server's public parameters through the group's own params).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn zkgroup_auth_credentials() {
    if server().is_empty() {
        return;
    }
    let url = server();
    let info: serde_json::Value = reqwest::get(format!("{url}/v1/info")).await.unwrap().json().await.unwrap();
    if info["zkgroup_public_params"].is_null() {
        eprintln!("server without zkgroup (Go server), skipping");
        return;
    }
    let ana = Client::new("ana");
    register_all(&url, &[&ana]).await;
    let a = ana
        .core
        .run(|i| async move {
            let group = zkgroup::groups::GroupSecretParams::generate([9u8; 32]);
            let p1 = i.zk_auth_presentation(&group).await?;
            let p2 = i.zk_auth_presentation(&group).await?;
            Ok((p1, p2))
        })
        .await
        .unwrap();
    let parsed = zkgroup::auth::AnyAuthCredentialPresentation::new(&a.0).unwrap();
    let group = zkgroup::groups::GroupSecretParams::generate([9u8; 32]);
    let aci = libsignal_core::Aci::from(uuid::Uuid::parse_str(&ana.core.run(|i| async move { i.me() }).await.unwrap()).unwrap());
    // The presentation carries our id encrypted under the group key (and nothing in clear).
    assert!(zkgroup::serialize(&parsed.get_aci_ciphertext()) == zkgroup::serialize(&group.encrypt_service_id(aci.into())));
    // Presentations are randomized: two of them cannot be linked by the server.
    assert_ne!(a.0, a.1);
}

/// A call offer our clients would produce (the core drops anything else before the UI).
fn offer_sdp() -> String {
    "v=0\r\no=- 1 2 IN IP4 127.0.0.1\r\ns=-\r\nt=0 0\r\nm=audio 9 UDP/TLS/RTP/SAVPF 111\r\nc=IN IP4 0.0.0.0\r\n\
     a=fingerprint:sha-256 4A:AD:B9:B1:3F:82:18:3B:54:02:12:DF:3E:5D:49:6B:19:E5:7C:AB:3D:B4:0A:3E:4E:A9:66:9F:12:E5:8A:01\r\n\
     a=setup:actpass\r\na=mid:0\r\na=rtpmap:111 opus/48000/2\r\n"
        .to_string()
}

async fn call_offer(from: &Client, to: &str, call_id: &str) {
    let req: chat_core::engine::CallSignalRequest = serde_json::from_value(serde_json::json!({
        "account_id": to, "call_id": call_id, "kind": "offer", "data": { "sdp": offer_sdp() }
    }))
    .unwrap();
    from.core.run(move |i| async move { i.call_signal(req).await }).await.unwrap();
}

fn saw_call(c: &Client, call_id: &str, kind: &str) -> bool {
    c.events.0.lock().unwrap().iter().any(|(n, v)| n == "call_signal" && v["call_id"] == call_id && v["kind"] == kind)
}

/// Message requests (engine/requests.rs): what a stranger sends is stored and shown, but
/// nothing is downloaded, acknowledged or rung until the user accepts, and strangers cannot
/// watch anyone's presence.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn message_requests() {
    if server().is_empty() {
        return;
    }
    let url = server();
    let (bob, eve, ana) = (Client::new("bob"), Client::new("eve"), Client::new("ana"));
    let acc = register_all(&url, &[&bob, &eve, &ana]).await;
    let b = acc[0].account_id.clone();

    // Eve, a stranger, writes to Bob with a picture.
    let ec = direct_conv(&eve, &acc[0].number).await;
    let mut png = Vec::new();
    image::RgbImage::from_fn(32, 32, |x, y| image::Rgb([x as u8 * 8, y as u8 * 8, 99]))
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    let req: SendMessageRequest = serde_json::from_value(serde_json::json!({
        "conversation_id": ec.id, "text": "from a stranger",
        "attachments": [{ "data_base64": base64_encode(&png), "file_name": "x.png", "mime": "image/png", "kind": "image" }]
    }))
    .unwrap();
    let sent = eve.core.run(move |i| async move { i.send_message(req).await }).await.unwrap();
    let got = bob.wait_message("from a stranger").await;
    let bc = got["conversation_id"].as_str().unwrap().to_string();
    assert!(got["attachments"][0]["thumbnail"].is_null(), "no thumbnail from a stranger for the UI to decode");
    let c = bc.clone();
    let conv = bob.core.run(move |i| async move { i.get_conversation(&c) }).await.unwrap();
    assert!(!conv.accepted && !conv.peer_is_contact, "a message request");
    eve.wait_status(&sent.id, "sent").await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(messages(&bob, &bc).await[0].attachments[0].download_state, "pending", "nothing downloads from a stranger");
    let sid = sent.id.clone();
    assert_eq!(eve.core.run(move |i| async move { i.get_message(&sid) }).await.unwrap().status, "sent", "no delivery receipt to a stranger");

    // Eve cannot watch Bob's presence: the core has no key to offer, and forged or old-style
    // subscriptions are ignored by the server.
    let bb = b.clone();
    eve.core
        .run(move |i| async move {
            i.watch_presence(vec![bb.clone()]);
            i.ws_send(serde_json::json!({ "type": "presence_subscribe", "account_ids": [bb] }));
            i.ws_send(serde_json::json!({ "type": "presence_subscribe",
                "subscriptions": [{ "account_id": bb, "access_key": base64_encode(&[0u8; 16]) }] }));
            Ok(())
        })
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_secs(2)).await;
    let watched = |c: &Client, who: &str| c.events.0.lock().unwrap().iter().any(|(n, v)| n == "presence" && v["account_id"] == who);
    assert!(!watched(&eve, &b), "a stranger must not see Bob's presence");

    // Her call does not ring: a missed call for Bob, "need_permission" for her.
    call_offer(&eve, &b, "req-call-1").await;
    let hangup = eve.wait_event("need_permission", |n, v| n == "call_signal" && v["call_id"] == "req-call-1" && v["kind"] == "hangup").await;
    assert_eq!(hangup["data"]["reason"], "need_permission");
    let c = bc.clone();
    bob.wait_event("missed call", move |n, v| {
        n == "message" && v["conversation_id"].as_str() == Some(c.as_str()) && v["message"]["system"]["type"] == "call"
    })
    .await;
    assert!(!saw_call(&bob, "req-call-1", "offer"), "a stranger's call must not reach the UI");

    // A stranger's flood is cut short: 3 calls a minute, the rest leave no trace.
    for i in 2..=5 {
        call_offer(&eve, &b, &format!("req-flood-{i}")).await;
    }
    eve.wait_event("third stranger call", |n, v| n == "call_signal" && v["call_id"] == "req-flood-3" && v["kind"] == "hangup").await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    for i in 4..=5 {
        assert!(!saw_call(&eve, &format!("req-flood-{i}"), "hangup"), "stranger offer {i} must be dropped");
    }
    let call_entries = |ms: &[Message]| ms.iter().filter(|m| m.system.as_ref().is_some_and(|s| s.kind == "call")).count();
    assert_eq!(call_entries(&messages(&bob, &bc).await), 3, "dropped offers leave no history");

    // Accepting: the held receipt goes out, the picture downloads, Eve gets Bob's profile key
    // (and with it his presence).
    let c = bc.clone();
    bob.core.run(move |i| async move { i.accept_conversation(&c).await }).await.unwrap();
    eve.wait_status(&sent.id, "delivered").await;
    let mid = got["id"].as_str().unwrap().to_string();
    bob.wait_event("held download", move |n, v| {
        n == "message" && v["message"]["id"].as_str() == Some(mid.as_str()) && v["message"]["attachments"][0]["download_state"] == "done"
    })
    .await;
    let bb = b.clone();
    eve.wait_event("bob's presence", move |n, v| n == "presence" && v["account_id"].as_str() == Some(bb.as_str()) && v["online"] == true)
        .await;

    // Now her calls ring, relayed (she is not in Bob's contacts), as often as she calls: the
    // limit is for strangers only.
    for i in 2..=7 {
        call_offer(&eve, &b, &format!("req-call-{i}")).await;
    }
    let offer = bob.wait_event("accepted call", |n, v| n == "call_signal" && v["call_id"] == "req-call-2" && v["kind"] == "offer").await;
    assert_eq!(offer["relay"], true, "calls with non-contacts go through the relay");
    for i in 3..=7 {
        let id = format!("req-call-{i}");
        bob.wait_event(&format!("accepted call {i}"), move |n, v| n == "call_signal" && v["call_id"] == id.as_str() && v["kind"] == "offer").await;
    }
    assert_eq!(call_entries(&messages(&bob, &bc).await), 3, "rung calls are recorded by the UI, not the core");

    // A contact's call rings directly (no relay needed).
    let an = acc[2].number.clone();
    bob.core.run(move |i| async move { i.add_contact_by_number(&an, None).await }).await.unwrap();
    call_offer(&ana, &b, "contact-call").await;
    let offer = bob.wait_event("contact call", |n, v| n == "call_signal" && v["call_id"] == "contact-call" && v["kind"] == "offer").await;
    assert_eq!(offer["relay"], false);
}

/// Do not disturb (engine/dnd.rs): an accepted peer's call does not ring and becomes a missed
/// Presence follows the user, not the connection: a device whose user is away (idle desktop,
/// app in the background) does not show the account online; last seen is when they left.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn presence_away() {
    if server().is_empty() {
        return;
    }
    let url = server();
    let (bob, ana) = (Client::new("bob"), Client::new("ana"));
    let acc = register_all(&url, &[&bob, &ana]).await;
    let b = acc[0].account_id.clone();
    let bc = direct_conv(&bob, &acc[1].number).await.id;
    let ac = direct_conv(&ana, &acc[0].number).await.id;
    // Messages both ways share the profile keys, and with them presence.
    send_text(&bob, &bc, "hi ana").await;
    ana.wait_message("hi ana").await;
    send_text(&ana, &ac, "hi bob").await;
    bob.wait_message("hi bob").await;
    ana.core.run(|i| async move { i.send_presence_subscription(); Ok(()) }).await.unwrap();
    let bb = b.clone();
    ana.wait_event("bob online", move |n, v| n == "presence" && v["account_id"].as_str() == Some(bb.as_str()) && v["online"] == true)
        .await;
    let count = |online: bool| {
        ana.events.0.lock().unwrap().iter().filter(|(n, v)| n == "presence" && v["account_id"] == b.as_str() && v["online"] == online).count()
    };
    let wait_more = |online: bool, before: usize| async move {
        for _ in 0..100 {
            if count(online) > before {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("no presence online={online} for bob");
    };

    // Bob steps away: offline, last seen when he left.
    let left_at = chat_core::util::now_ms() - 60_000;
    bob.core.run(move |i| async move { i.set_presence_active(false, Some(left_at)); Ok(()) }).await.unwrap();
    let bb = b.clone();
    ana.wait_event("bob away", move |n, v| {
        n == "presence" && v["account_id"].as_str() == Some(bb.as_str()) && v["online"] == false && v["last_seen"].as_i64() == Some(left_at)
    })
    .await;

    // Back at his device: online again.
    let onlines = count(true);
    bob.core.run(|i| async move { i.set_presence_active(true, None); Ok(()) }).await.unwrap();
    wait_more(true, onlines).await;

    // Away again; a new subscription sees him offline (still connected), with his last seen.
    let offlines = count(false);
    bob.core.run(move |i| async move { i.set_presence_active(false, Some(left_at)); Ok(()) }).await.unwrap();
    wait_more(false, offlines).await;
    let offlines = count(false);
    ana.core.run(|i| async move { i.send_presence_subscription(); Ok(()) }).await.unwrap();
    wait_more(false, offlines).await;
    let last = ana.events.0.lock().unwrap().iter().rev().find(|(n, v)| n == "presence" && v["account_id"] == b.as_str()).map(|(_, v)| v.clone()).unwrap();
    assert_eq!(last["online"], false, "{last}");
    assert_eq!(last["last_seen"].as_i64(), Some(left_at), "{last}");
}

/// call, nothing goes back to the caller; once it is off, calls ring again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn do_not_disturb() {
    if server().is_empty() {
        return;
    }
    let url = server();
    let (bob, ana) = (Client::new("bob"), Client::new("ana"));
    let acc = register_all(&url, &[&bob, &ana]).await;
    let b = acc[0].account_id.clone();
    let bc = direct_conv(&bob, &acc[1].number).await.id;
    direct_conv(&ana, &acc[0].number).await;

    let st = bob.core.run(|i| async move { i.set_dnd(i64::MAX).await }).await.unwrap();
    assert_eq!(st.until, chat_core::engine::DND_FOREVER);
    bob.wait_event("dnd on", |n, v| n == "dnd_changed" && v["until"].as_i64() == Some(i64::MAX)).await;

    call_offer(&ana, &b, "dnd-call").await;
    let c = bc.clone();
    let missed = bob
        .wait_event("missed call", move |n, v| {
            n == "message" && v["conversation_id"].as_str() == Some(c.as_str()) && v["message"]["system"]["type"] == "call"
        })
        .await;
    assert!(missed["message"]["system"]["value"].as_str().is_some_and(|v| v.contains("missed")), "{missed}");
    tokio::time::sleep(Duration::from_secs(2)).await;
    let any_signal = |c: &Client, id: &str| c.events.0.lock().unwrap().iter().any(|(n, v)| n == "call_signal" && v["call_id"] == id);
    assert!(!any_signal(&bob, "dnd-call"), "no ringing during do not disturb");
    assert!(!any_signal(&ana, "dnd-call"), "nothing goes back to the caller (no busy, hangup or ringing)");

    let st = bob.core.run(|i| async move { i.set_dnd(0).await }).await.unwrap();
    assert_eq!(st.until, 0);
    call_offer(&ana, &b, "after-dnd").await;
    bob.wait_event("call rings again", |n, v| n == "call_signal" && v["call_id"] == "after-dnd" && v["kind"] == "offer").await;
}

/// Call privacy (engine/call_settings.rs). Calls turned off: an accepted peer's offer does not
/// ring and leaves no history, and the caller is told `hangup` / `calls_disabled`; we cannot
/// call either. Relay-only: incoming calls say `relay` even from contacts, and what we send
/// carries relay candidates only.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn call_settings() {
    if server().is_empty() {
        return;
    }
    let url = server();
    let (bob, ana) = (Client::new("bob"), Client::new("ana"));
    let acc = register_all(&url, &[&bob, &ana]).await;
    let (b, a) = (acc[0].account_id.clone(), acc[1].account_id.clone());
    let bc = direct_conv(&bob, &acc[1].number).await.id;
    let ac = direct_conv(&ana, &acc[0].number).await.id;
    let patch = |allow: Option<bool>, relay: Option<bool>| -> chat_core::engine::CallSettingsPatch {
        serde_json::from_value(serde_json::json!({ "allow_calls": allow, "relay_only": relay })).unwrap()
    };

    let st = bob.core.run(|i| async move { Ok(i.get_call_settings()) }).await.unwrap();
    assert!(st.allow_calls && !st.relay_only, "defaults: calls on, direct");
    let p = patch(Some(false), None);
    let st = bob.core.run(move |i| async move { i.set_call_settings(p).await }).await.unwrap();
    assert!(!st.allow_calls);
    bob.wait_event("calls off", |n, v| n == "call_settings_changed" && v["allow_calls"] == false).await;

    call_offer(&ana, &b, "off-call").await;
    let hangup = ana.wait_event("calls_disabled", |n, v| n == "call_signal" && v["call_id"] == "off-call" && v["kind"] == "hangup").await;
    assert_eq!(hangup["data"]["reason"], "calls_disabled");
    assert_eq!(hangup["from_me"], false);
    tokio::time::sleep(Duration::from_secs(1)).await;
    let any_signal = |c: &Client, id: &str| c.events.0.lock().unwrap().iter().any(|(n, v)| n == "call_signal" && v["call_id"] == id);
    assert!(!any_signal(&bob, "off-call"), "no ringing while calls are off");
    let calls = messages(&bob, &bc).await.into_iter().filter(|m| m.system.as_ref().is_some_and(|s| s.kind == "call")).count();
    assert_eq!(calls, 0, "no history either");
    // Nor can we call out.
    let aa = a.clone();
    let out = bob.core.run(move |i| async move {
        let req: chat_core::engine::CallSignalRequest = serde_json::from_value(serde_json::json!({
            "account_id": aa, "call_id": "off-out", "kind": "offer", "data": { "sdp": offer_sdp() }
        }))
        .unwrap();
        i.call_signal(req).await
    });
    assert!(out.await.is_err(), "placing a call while calls are off is refused");
    // The caller's own record of it.
    let rec: chat_core::engine::RecordCallRequest = serde_json::from_value(serde_json::json!({
        "conversation_id": ac, "call_id": "off-call",
        "caller_account_id": a, "status": "unavailable", "started_at": 1
    }))
    .unwrap();
    ana.core.run(move |i| async move { i.record_call(rec) }).await.unwrap();

    // Turned on again: calls ring (directly, Ana is a contact).
    let p = patch(Some(true), None);
    bob.core.run(move |i| async move { i.set_call_settings(p).await }).await.unwrap();
    call_offer(&ana, &b, "on-call").await;
    let offer = bob.wait_event("call rings again", |n, v| n == "call_signal" && v["call_id"] == "on-call" && v["kind"] == "offer").await;
    assert_eq!(offer["relay"], false);

    // Relay-only on Bob's side: even a contact's call is relayed for him.
    let p = patch(None, Some(true));
    let st = bob.core.run(move |i| async move { i.set_call_settings(p).await }).await.unwrap();
    assert!(st.allow_calls && st.relay_only);
    call_offer(&ana, &b, "relay-call").await;
    let offer = bob.wait_event("relayed call", |n, v| n == "call_signal" && v["call_id"] == "relay-call" && v["kind"] == "offer").await;
    assert_eq!(offer["relay"], true, "relay-only is reported to the UI");

    // What Bob sends carries relay candidates only (in the SDP and in ICE signals).
    let host = "candidate:1 1 udp 2122260223 192.168.7.20 50000 typ host generation 0";
    let relay = "candidate:3 1 udp 41885439 198.51.100.9 3478 typ relay raddr 0.0.0.0 rport 0";
    let sdp = offer_sdp().replace("a=mid:0\r\n", &format!("a=mid:0\r\na={host}\r\na={relay}\r\n"));
    let aa = a.clone();
    bob.core
        .run(move |i| async move {
            for (kind, data) in [
                ("answer", serde_json::json!({ "sdp": sdp })),
                ("ice", serde_json::json!({ "candidates": [{ "candidate": host, "sdpMid": "0" }, { "candidate": relay, "sdpMid": "0" }] })),
                ("ice", serde_json::json!({ "candidates": [{ "candidate": host, "sdpMid": "0" }] })),
            ] {
                let req: chat_core::engine::CallSignalRequest = serde_json::from_value(serde_json::json!({
                    "account_id": aa, "call_id": "relay-call", "kind": kind, "data": data
                }))
                .unwrap();
                i.call_signal(req).await?;
            }
            Ok(())
        })
        .await
        .unwrap();
    let answer = ana.wait_event("relay-only answer", |n, v| n == "call_signal" && v["call_id"] == "relay-call" && v["kind"] == "answer").await;
    let got = answer["data"]["sdp"].as_str().unwrap();
    assert!(!got.contains("192.168.7.20") && got.contains("typ relay"), "{got}");
    let ice = ana.wait_event("relay-only ice", |n, v| n == "call_signal" && v["call_id"] == "relay-call" && v["kind"] == "ice").await;
    assert_eq!(ice["data"]["candidates"], serde_json::json!([{ "candidate": relay, "sdpMid": "0" }]));
    tokio::time::sleep(Duration::from_secs(1)).await;
    let ices = ana.events.0.lock().unwrap().iter().filter(|(n, v)| n == "call_signal" && v["call_id"] == "relay-call" && v["kind"] == "ice").count();
    assert_eq!(ices, 1, "a signal with host candidates only is not sent at all");
}

/// Waits (longer than `wait_event`: retries back off) for an event matching `pred`.
async fn wait_event_long(c: &Client, what: &str, secs: u64, pred: impl Fn(&str, &Value) -> bool) -> Value {
    for _ in 0..secs * 10 {
        if let Some((_, v)) = c.events.0.lock().unwrap().iter().find(|(n, v)| pred(n, v)) {
            return v.clone();
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("{}: timed out waiting for {what}", c.name);
}

/// The peer logs out of their only device (the account has no device at all): messages to them
/// stay pending with the reason "no_devices" (our other devices still get their copy) and are
/// delivered on their own once the peer logs in again. A deleted account fails for good.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn peer_without_devices() {
    if server().is_empty() {
        return;
    }
    let _ = env_logger::builder().try_init();
    let url = server();
    let (ana, bob, cora) = (Client::new("ana"), Client::new("bob"), Client::new("cora"));
    let acc = register_all(&url, &[&ana, &bob, &cora]).await;
    let ac = direct_conv(&ana, &acc[1].number).await.id;
    let first = send_text(&ana, &ac, "before logout").await;
    let got = bob.wait_message("before logout").await;
    let bc = got["conversation_id"].as_str().unwrap().to_string();
    bob.core.run(move |i| async move { i.accept_conversation(&bc).await }).await.unwrap();
    ana.wait_status(&first.id, "delivered").await;

    // Ana's second device, to check the sync copy.
    let laptop = Client::new("ana-laptop");
    let (u, n) = (url.clone(), acc[0].number.clone());
    laptop.core.run(move |i| async move { i.login(&u, &n, "correct horse battery", "laptop", None).await }).await.unwrap();
    laptop.wait_online().await;

    bob.core.run(|i| async move { i.logout().await }).await.unwrap();
    drop(bob);

    let waiting = send_text(&ana, &ac, "while you were away").await;
    let id = waiting.id.clone();
    let pending = ana
        .wait_event("no_devices reason", move |n, v| {
            n == "message" && v["message"]["id"].as_str() == Some(id.as_str()) && v["message"]["status_reason"] == "no_devices"
        })
        .await;
    assert_eq!(pending["message"]["status"], "pending", "{pending}");
    // Our own other device got its copy although the peer has none.
    laptop.wait_message("while you were away").await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let m = ana.core.run({
        let id = waiting.id.clone();
        move |i| async move { i.get_message(&id) }
    })
    .await
    .unwrap();
    assert_eq!((m.status.as_str(), m.status_reason.as_deref()), ("pending", Some("no_devices")), "still waiting, not failed");

    // Bob is back on a new device: the message goes out by itself.
    let bob2 = Client::new("bob-again");
    let (u, n) = (url.clone(), acc[1].number.clone());
    bob2.core.run(move |i| async move { i.login(&u, &n, "correct horse battery", "bob again", None).await }).await.unwrap();
    bob2.wait_online().await;
    wait_event_long(&bob2, "the waiting message", 90, |n, v| n == "message" && v["message"]["text"] == "while you were away").await;
    let id = waiting.id.clone();
    let sent = wait_event_long(&ana, "sent", 30, move |n, v| {
        n == "message"
            && v["message"]["id"].as_str() == Some(id.as_str())
            && matches!(v["message"]["status"].as_str(), Some("sent" | "delivered" | "read"))
    })
    .await;
    assert!(sent["message"]["status_reason"].is_null(), "the reason goes once sent: {sent}");

    // A deleted account: a clear, final failure.
    let cc = direct_conv(&ana, &acc[2].number).await.id;
    cora.core.run(|i| async move { i.delete_account("correct horse battery").await }).await.unwrap();
    let gone = send_text(&ana, &cc, "hello?").await;
    let id = gone.id.clone();
    let failed = ana
        .wait_event("account_gone", move |n, v| {
            n == "message" && v["message"]["id"].as_str() == Some(id.as_str()) && v["message"]["status"] == "failed"
        })
        .await;
    assert_eq!(failed["message"]["status_reason"], "account_gone", "{failed}");
}
