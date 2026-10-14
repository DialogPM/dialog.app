//! Media retention (engine/retention.rs): photos, voice notes and files are erased from every
//! device after the period (30 days by default; the clock is shortened to seconds here), the
//! message rows stay with an "expired" stub, nothing can be downloaded again, and a vault
//! restore on a new device does not bring the media back. The media check removes stray files
//! and stale decrypted copies; a shorter period applies at once.
//!
//! `CHATAPP_E2E_SERVER=http://127.0.0.1:8081 cargo test -p chat-core --features insecure-test-hooks --test e2e_media_retention`
//! (its own test binary: the shortened limit applies to the whole process).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use chat_core::model::*;
use chat_core::{Core, CoreConfig, ErrorCode, EventSink};
use serde_json::{json, Value};

/// The 30-day period in this test (one "day" is 0.4 s).
const LIMIT: Duration = Duration::from_secs(12);

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
            db_key: [5u8; 32],
            default_server_url: server(),
            platform: "cli".into(),
            events: events.clone(),
        })
        .unwrap();
        Self { name, core, events, _dir: dir }
    }

    async fn wait(&self, what: &str, pred: impl Fn(&str, &Value) -> bool) -> Value {
        for _ in 0..600 {
            if let Some((_, v)) = self.events.0.lock().unwrap().iter().find(|(n, v)| pred(n, v)) {
                return v.clone();
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("{}: timed out waiting for {what}", self.name);
    }

    async fn wait_message(&self, id: &str, what: &str, pred: impl Fn(&Value) -> bool) -> Value {
        let id = id.to_string();
        self.wait(what, move |n, v| n == "message" && v["message"]["id"] == id.as_str() && pred(&v["message"]))
            .await["message"]
            .clone()
    }

    async fn message(&self, id: &str) -> Message {
        let id = id.to_string();
        self.core.run(move |i| async move { i.get_message(&id) }).await.unwrap()
    }

    /// The raw attachments JSON stored in the database.
    async fn attachments_json(&self, id: &str) -> String {
        let id = id.to_string();
        self.core
            .run(move |i| async move {
                Ok(i.db.borrow().conn.query_row("SELECT attachments_json FROM messages WHERE id = ?1", [&id], |r| r.get::<_, String>(0))?)
            })
            .await
            .unwrap()
    }

    async fn download(&self, mid: &str, aid: &str) -> chat_core::Result<()> {
        let (mid, aid) = (mid.to_string(), aid.to_string());
        self.core.run(move |i| async move { i.download_attachment(&mid, &aid).await }).await
    }
}

fn server() -> String {
    std::env::var("CHATAPP_E2E_SERVER").unwrap_or_default()
}

fn b64(d: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(d)
}

fn all_done(m: &Value) -> bool {
    m["attachments"].as_array().is_some_and(|a| !a.is_empty() && a.iter().all(|x| x["download_state"] == "done"))
}

fn all_expired(m: &Value) -> bool {
    m["attachments"].as_array().is_some_and(|a| !a.is_empty() && a.iter().all(|x| x["download_state"] == "expired"))
}

fn assert_stub(json: &str, keys: &[&str]) {
    let atts: Vec<Value> = serde_json::from_str(json).unwrap();
    assert!(!atts.is_empty(), "attachments dropped: {json}");
    for a in &atts {
        assert_eq!(a["download_state"], "expired", "{json}");
        assert!(a["expired_at"].is_i64(), "{json}");
        for field in ["key", "digest", "cdn_id", "thumbnail", "local_path", "waveform"] {
            assert!(a[field].is_null(), "{field} kept: {json}");
        }
    }
    for k in keys {
        assert!(!json.contains(k), "attachment key still stored: {json}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn media_is_erased_after_the_retention_limit() {
    if server().is_empty() {
        eprintln!("CHATAPP_E2E_SERVER not set, skipping");
        return;
    }
    chat_core::engine::retention::set_media_retention_for_tests(LIMIT);
    let url = server();
    let (alice, bob) = (Client::new("alice"), Client::new("bob"));
    let mut accounts = Vec::new();
    for c in [&alice, &bob] {
        let u = url.clone();
        let r = c.core.run(move |i| async move { i.register(&u, "retention test pw 1", "test").await }).await.unwrap();
        c.wait("online", |n, v| n == "connection" && v.as_str() == Some("online")).await;
        accounts.push(r.account);
    }
    let (bnum, bid) = (accounts[1].number.clone(), accounts[1].account_id.clone());
    alice.core.run(move |i| async move { i.add_contact_by_number(&bnum, Some("Bob".into())).await }).await.unwrap();
    let conv = alice.core.run(move |i| async move { i.open_direct_conversation(&bid).await }).await.unwrap().id;

    // A voice note (no text), and a photo + document with a caption.
    let voice: Vec<u8> = (0..20_000u32).map(|i| (i * 7 % 251) as u8).collect();
    let mut png = Vec::new();
    image::RgbImage::from_fn(64, 64, |x, y| image::Rgb([x as u8 * 4, y as u8 * 4, 7]))
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    let doc: Vec<u8> = b"retention test document ".iter().cycle().take(50_000).copied().collect();
    let send = |req: Value| {
        let req: SendMessageRequest = serde_json::from_value(req).unwrap();
        alice.core.run(move |i| async move { i.send_message(req).await })
    };
    let sent_voice = send(json!({ "conversation_id": conv, "attachments": [
        { "data_base64": b64(&voice), "file_name": "voice-1.webm", "mime": "audio/webm", "kind": "voice", "duration_ms": 2000,
          "waveform": [0.1, 0.5, 0.9] }
    ]}))
    .await
    .unwrap();
    let sent_pics = send(json!({ "conversation_id": conv, "text": "pics", "attachments": [
        { "data_base64": b64(&png), "file_name": "holiday.png", "mime": "image/png", "kind": "image" },
        { "data_base64": b64(&doc), "file_name": "contract.txt", "mime": "text/plain", "kind": "file" }
    ]}))
    .await
    .unwrap();
    let mut alice_files: Vec<String> = sent_voice.attachments.iter().chain(&sent_pics.attachments).map(|a| a.local_path.clone().unwrap()).collect();

    // Bob receives and downloads everything.
    let mut bob_files = Vec::new();
    let mut keys = Vec::new();
    for m in [&sent_voice, &sent_pics] {
        bob.wait_message(&m.id, "message", |_| true).await;
        for a in &m.attachments {
            bob.download(&m.id, &a.id).await.unwrap();
        }
        let got = bob.wait_message(&m.id, "downloaded", all_done).await;
        bob_files.extend(got["attachments"].as_array().unwrap().iter().map(|a| a["local_path"].as_str().unwrap().to_string()));
        let json = bob.attachments_json(&m.id).await;
        let atts: Vec<Value> = serde_json::from_str(&json).unwrap();
        keys.extend(atts.iter().map(|a| a["key"].as_str().unwrap().to_string()));
        assert!(atts.iter().any(|a| a["thumbnail"].is_string()) || m.id == sent_voice.id);
    }
    alice_files.retain(|f| !f.is_empty());
    assert!(alice_files.iter().chain(&bob_files).all(|f| std::path::Path::new(f).exists()));

    // The media check: a stray file and a stale decrypted copy go, live attachments stay.
    let media_dir = std::path::Path::new(&bob_files[0]).parent().unwrap().to_path_buf();
    let temp_dir = media_dir.parent().unwrap().join("media-temp");
    std::fs::create_dir_all(&temp_dir).unwrap();
    let stray = media_dir.join("stray.bin");
    let stale_copy = temp_dir.join("opened-copy.png");
    std::fs::write(&stray, b"leftover").unwrap();
    std::fs::write(&stale_copy, &png).unwrap();
    let old = std::time::SystemTime::now() - Duration::from_secs(600);
    for f in [&stray, &stale_copy] {
        std::fs::File::options().write(true).open(f).unwrap().set_modified(old).unwrap();
    }
    let check = bob.core.run(|i| async move { i.run_media_check().await }).await.unwrap();
    assert!(check.removed >= 2, "{check:?}");
    assert!(!stray.exists() && !stale_copy.exists(), "stray files survived the media check");
    assert!(bob_files.iter().all(|f| std::path::Path::new(f).exists()), "the check removed a live attachment");
    let last = bob.core.run(|i| async move { Ok(i.get_media_retention().last_check) }).await.unwrap();
    assert_eq!(last.map(|c| c.checked_at), Some(check.checked_at));

    // After the limit: erased on both sides, without anyone doing anything.
    for c in [&alice, &bob] {
        for m in [&sent_voice, &sent_pics] {
            c.wait_message(&m.id, "media expired", all_expired).await;
        }
    }
    for f in alice_files.iter().chain(&bob_files) {
        assert!(!std::path::Path::new(f).exists(), "{f} still on disk");
    }
    let key_refs: Vec<&str> = keys.iter().map(String::as_str).collect();
    for c in [&alice, &bob] {
        for m in [&sent_voice, &sent_pics] {
            assert_stub(&c.attachments_json(&m.id).await, &key_refs);
            // The row stays, with a placeholder.
            let msg = c.message(&m.id).await;
            assert_eq!(msg.kind, m.kind);
            assert!(msg.attachments.iter().all(|a| a.expired_at.is_some() && a.thumbnail.is_none() && a.local_path.is_none()));
        }
        assert_eq!(c.message(&sent_pics.id).await.text.as_deref(), Some("pics"), "the caption stays");
        assert!(c.message(&sent_voice.id).await.text.is_none());
    }
    // Only documents keep their name.
    let pics = bob.message(&sent_pics.id).await;
    assert_eq!(pics.attachments[0].file_name, "");
    assert_eq!(pics.attachments[1].file_name, "contract.txt");
    // The chat list shows the placeholder for the last message.
    let convs = bob.core.run(|i| async move { i.list_conversations() }).await.unwrap();
    assert!(convs.iter().any(|c| c.last_message.as_ref().is_some_and(|m| m.id == sent_pics.id && m.media_expired)));

    // No way back: download, save and forward all fail with "expired".
    let err = bob.download(&sent_voice.id, &sent_voice.attachments[0].id).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::Gone);
    let dest = tempfile::tempdir().unwrap();
    let out = dest.path().join("x").to_string_lossy().to_string();
    let (mid, aid) = (sent_pics.id.clone(), sent_pics.attachments[1].id.clone());
    let err = bob.core.run(move |i| async move { i.save_attachment(&mid, &aid, &out).await }).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::Gone);
    let (ids, convs) = (vec![sent_pics.id.clone()], vec![conv.clone()]);
    let err = alice.core.run(move |i| async move { i.forward_messages(&ids, &convs) }).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::Gone);

    // A shorter period applies at once: with 1 day (0.4 s here), a second-old photo goes now.
    let fresh = send(json!({ "conversation_id": conv, "attachments": [
        { "data_base64": b64(&png), "file_name": "fresh.png", "mime": "image/png", "kind": "image" }
    ]}))
    .await
    .unwrap();
    let fresh_file = fresh.attachments[0].local_path.clone().unwrap();
    assert!(std::path::Path::new(&fresh_file).exists());
    tokio::time::sleep(Duration::from_secs(1)).await;
    let bad = alice.core.run(|i| async move { i.set_media_retention(45).await }).await.unwrap_err();
    assert_eq!(bad.code, ErrorCode::InvalidInput);
    let st = alice.core.run(|i| async move { i.set_media_retention(1).await }).await.unwrap();
    assert_eq!(st.days, 1);
    assert!(!std::path::Path::new(&fresh_file).exists(), "media older than the new period kept");
    assert!(alice.message(&fresh.id).await.attachments.iter().all(|a| a.download_state == "expired"));

    // A new device restoring the history from the vault gets the stubs, not the media.
    tokio::time::sleep(Duration::from_secs(3)).await; // let the vault flush
    let alice2 = Client::new("alice2");
    let (u, anum) = (url.clone(), accounts[0].number.clone());
    alice2.core.run(move |i| async move { i.login(&u, &anum, "retention test pw 1", "second", None).await }).await.unwrap();
    alice2.wait("history restored", |n, v| n == "history_sync" && v["done"] == true).await;
    for m in [&sent_voice, &sent_pics] {
        let restored = alice2.message(&m.id).await;
        assert_eq!(restored.attachments.len(), m.attachments.len(), "restored without its attachment stubs");
        assert!(restored.attachments.iter().all(|a| a.download_state == "expired" && a.local_path.is_none()));
        assert_stub(&alice2.attachments_json(&m.id).await, &key_refs);
        let err = alice2.download(&m.id, &m.attachments[0].id).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::Gone);
    }
    // The chosen period reached the new device with the settings.
    let days = alice2.core.run(|i| async move { Ok(i.get_media_retention().days) }).await.unwrap();
    assert_eq!(days, 1);
}
