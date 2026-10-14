//! End-to-end tests of view-once media, photos and voice (engine/view_once.rs) and profile key rotation on
//! block (engine/profile_key.rs).
//!
//! Run with a server listening (rate limits disabled), e.g.:
//! `CHATAPP_E2E_SERVER=http://127.0.0.1:8080 cargo test -p chat-core --test e2e_privacy`

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

    async fn register(&self) -> AccountInfo {
        let u = server();
        let r = self.core.run(move |i| async move { i.register(&u, PASSWORD, "test").await }).await.unwrap();
        self.wait_online().await;
        r.account
    }

    async fn login(&self, number: &str) -> AccountInfo {
        let (u, n) = (server(), number.to_string());
        let a = self.core.run(move |i| async move { i.login(&u, &n, PASSWORD, "second", None).await }).await.unwrap();
        self.wait_online().await;
        a
    }

    /// A message event for message `id` matching `pred`.
    async fn wait_msg(&self, what: &str, id: &str, pred: impl Fn(&Value) -> bool) -> Value {
        let id = id.to_string();
        self.wait_event(what, move |n, v| n == "message" && v["message"]["id"].as_str() == Some(id.as_str()) && pred(&v["message"]))
            .await["message"]
            .clone()
    }

    async fn set_name(&self, name: &str) {
        let n = name.to_string();
        self.core
            .run(move |i| async move {
                i.set_profile(SetProfileRequest { name: Some(n), about: None, avatar_path: None, avatar_data_base64: None }).await
            })
            .await
            .unwrap();
    }

    /// Adds `number` as a contact and opens the direct chat.
    async fn chat_with(&self, number: &str) -> Conversation {
        let n = number.to_string();
        let c = self.core.run(move |i| async move { i.add_contact_by_number(&n, None).await }).await.unwrap();
        self.core.run(move |i| async move { i.open_direct_conversation(&c.account_id).await }).await.unwrap()
    }

    async fn send(&self, conv: &str, body: Value) -> chat_core::Result<Message> {
        let mut req = body;
        req["conversation_id"] = Value::String(conv.to_string());
        let req: SendMessageRequest = serde_json::from_value(req).unwrap();
        self.core.run(move |i| async move { i.send_message(req).await }).await
    }

    async fn stored_contact_key(&self, account_id: &str) -> Option<Vec<u8>> {
        let a = account_id.to_string();
        self.core
            .run(move |i| async move {
                Ok(i.db.borrow().conn.query_row("SELECT profile_key FROM contacts WHERE account_id = ?1", [&a], |r| r.get(0)).ok().flatten())
            })
            .await
            .unwrap()
    }

    async fn profile_key(&self) -> [u8; 32] {
        self.core.run(|i| async move { i.profile_key() }).await.unwrap()
    }
}

fn server() -> String {
    std::env::var("CHATAPP_E2E_SERVER").unwrap_or_default()
}

fn b64(data: &[u8]) -> String {
    chat_core::crypto::b64(data)
}

fn png(seed: u8) -> Vec<u8> {
    let mut out = Vec::new();
    image::RgbImage::from_fn(40, 30, |x, y| image::Rgb([x as u8 * 6, y as u8 * 8, seed]))
        .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .unwrap();
    out
}

async fn poll<F, Fut>(what: &str, mut f: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    for _ in 0..300 {
        if f().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("timed out waiting for {what}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn view_once_media() {
    if server().is_empty() {
        eprintln!("CHATAPP_E2E_SERVER not set, skipping");
        return;
    }
    let (alice, bob) = (Client::new("alice"), Client::new("bob"));
    let a = alice.register().await;
    let b = bob.register().await;
    let bob2 = Client::new("bob2");
    bob2.login(&b.number).await;
    let ac = alice.chat_with(&b.number).await;
    bob.chat_with(&a.number).await;

    // Only one photo or video, without text.
    let pic = png(1);
    let bad = alice
        .send(&ac.id, serde_json::json!({ "text": "caption", "view_once": true,
            "attachments": [{ "data_base64": b64(&pic), "file_name": "p.png", "mime": "image/png", "kind": "image" }] }))
        .await;
    assert!(bad.is_err(), "view-once with text is refused");
    let bad = alice
        .send(&ac.id, serde_json::json!({ "view_once": true,
            "attachments": [{ "data_base64": b64(b"hello"), "file_name": "a.txt", "mime": "text/plain", "kind": "file" }] }))
        .await;
    assert!(bad.is_err(), "view-once file is refused");

    let sent = alice
        .send(&ac.id, serde_json::json!({ "view_once": true,
            "attachments": [{ "data_base64": b64(&pic), "file_name": "p.png", "mime": "image/png", "kind": "image" }] }))
        .await
        .unwrap();
    assert_eq!(sent.view_once.as_deref(), Some("unopened"));
    assert!(sent.attachments[0].local_path.is_none() && sent.attachments[0].thumbnail.is_none());
    let id = sent.id.clone();

    // Bob gets a placeholder: downloaded, but neither path nor preview for the UI.
    let got = bob
        .wait_msg("view-once downloaded", &id, |m| m["attachments"][0]["download_state"] == "done")
        .await;
    assert_eq!(got["view_once"], "unopened");
    assert!(got["attachments"][0]["local_path"].is_null() && got["attachments"][0]["thumbnail"].is_null(), "{got}");
    // The sender keeps no copy once it is sent.
    alice.wait_msg("sent", &id, |m| m["status"] != "pending").await;
    let i2 = id.clone();
    poll("sender copy erased", || {
        let i2 = i2.clone();
        let core = alice.core.clone();
        async move {
            core.run(move |i| async move { Ok(i.message_row(&i2)?.unwrap().attachments[0].local_path.is_none()) }).await.unwrap()
        }
    })
    .await;

    // No forwarding, no saving, not in the media gallery.
    let (i3, conv3) = (id.clone(), got["conversation_id"].as_str().unwrap().to_string());
    let c3 = conv3.clone();
    assert!(bob.core.run(move |i| async move { i.forward_messages(&[i3], &[c3]) }).await.is_err());
    let i3 = id.clone();
    let att_id = got["attachments"][0]["id"].as_str().unwrap().to_string();
    let a3 = att_id.clone();
    assert!(bob.core.run(move |i| async move { i.save_attachment(&i3, &a3, "/tmp/should-not-exist.png").await }).await.is_err());
    let c3 = conv3.clone();
    assert!(bob.core.run(move |i| async move { i.list_media(&c3, "media", None) }).await.unwrap().is_empty());

    // Opened once: the media is there, then gone.
    let i4 = id.clone();
    let att = bob.core.run(move |i| async move { i.open_view_once(&i4).await }).await.unwrap();
    let path = att.local_path.clone().expect("viewer gets the file");
    let p = path.clone();
    let bytes = bob.core.run(move |i| async move { i.read_media_file(std::path::Path::new(&p)).await }).await.unwrap();
    assert_eq!(bytes, pic);
    let i4 = id.clone();
    assert!(bob.core.run(move |i| async move { i.open_view_once(&i4).await }).await.is_err(), "only once");
    let i4 = id.clone();
    bob.core.run(move |i| async move { i.view_once_done(&i4) }).await.unwrap();
    assert!(!std::path::Path::new(&path).exists(), "file deleted after viewing");
    let i4 = id.clone();
    let row = bob.core.run(move |i| async move { Ok(i.message_row(&i4)?.unwrap()) }).await.unwrap();
    assert!(row.attachments[0].key.is_none() && row.attachments[0].cdn_id.is_none(), "key forgotten");
    assert_eq!(bob.core.run({ let i4 = id.clone(); move |i| async move { i.get_message(&i4) } }).await.unwrap().view_once.as_deref(), Some("opened"));

    // Bob's other device and the sender learn it was opened.
    bob2.wait_msg("opened on bob2", &id, |m| m["view_once"] == "opened").await;
    let i5 = id.clone();
    let row2 = bob2.core.run(move |i| async move { Ok(i.message_row(&i5)?.unwrap()) }).await.unwrap();
    assert!(row2.attachments[0].local_path.is_none() && row2.attachments[0].key.is_none(), "erased on bob2");
    alice.wait_msg("opened for sender", &id, |m| m["view_once"] == "opened").await;

    // Unopened media expires after 30 days.
    let pic2 = png(2);
    let sent2 = alice
        .send(&ac.id, serde_json::json!({ "view_once": true,
            "attachments": [{ "data_base64": b64(&pic2), "file_name": "q.png", "mime": "image/png", "kind": "image" }] }))
        .await
        .unwrap();
    let id2 = sent2.id.clone();
    bob.wait_msg("second downloaded", &id2, |m| m["attachments"][0]["download_state"] == "done").await;
    let i6 = id2.clone();
    let expired = bob
        .core
        .run(move |i| async move {
            let old = chat_core::util::now_ms() - 31 * 24 * 3600 * 1000;
            i.db.borrow().conn.execute("UPDATE messages SET received_at = ?2 WHERE id = ?1", rusqlite::params![i6, old])?;
            i.view_once_housekeeping(false)?;
            i.get_message(&i6)
        })
        .await
        .unwrap();
    assert_eq!(expired.view_once.as_deref(), Some("expired"));
    let i6 = id2.clone();
    assert!(bob.core.run(move |i| async move { i.open_view_once(&i6).await }).await.is_err());
}

/// A short mono 16-bit PCM WAV (a voice note as the desktop app records it).
fn wav(seed: u8) -> Vec<u8> {
    let samples: Vec<i16> = (0..8000).map(|i| (((i * (seed as i32 + 3)) % 200) as i16 - 100) * 50).collect();
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::new();
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&16_000u32.to_le_bytes());
    out.extend_from_slice(&32_000u32.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn view_once_voice() {
    if server().is_empty() {
        eprintln!("CHATAPP_E2E_SERVER not set, skipping");
        return;
    }
    let (alice, bob) = (Client::new("alice"), Client::new("bob"));
    let a = alice.register().await;
    let b = bob.register().await;
    let bob2 = Client::new("bob2");
    bob2.login(&b.number).await;
    let ac = alice.chat_with(&b.number).await;
    bob.chat_with(&a.number).await;

    let audio = wav(1);
    let voice = |data: &[u8]| {
        serde_json::json!({ "data_base64": b64(data), "file_name": "voice.wav", "mime": "audio/wav", "kind": "voice",
            "duration_ms": 500, "waveform": [0.2, 0.8, 0.5] })
    };
    // A single voice note, alone: not with text, not two of them.
    let bad = alice.send(&ac.id, serde_json::json!({ "text": "listen", "view_once": true, "attachments": [voice(&audio)] })).await;
    assert!(bad.is_err(), "view-once voice with text is refused");
    let bad = alice.send(&ac.id, serde_json::json!({ "view_once": true, "attachments": [voice(&audio), voice(&wav(2))] })).await;
    assert!(bad.is_err(), "two view-once attachments are refused");

    let sent = alice.send(&ac.id, serde_json::json!({ "view_once": true, "attachments": [voice(&audio)] })).await.unwrap();
    assert_eq!(sent.view_once.as_deref(), Some("unopened"));
    assert_eq!(sent.kind, "voice");
    assert!(sent.attachments[0].local_path.is_none());
    let id = sent.id.clone();

    // Bob gets a placeholder: downloaded, nothing playable for the UI.
    let got = bob
        .wait_msg("view-once voice downloaded", &id, |m| m["attachments"][0]["download_state"] == "done")
        .await;
    assert_eq!(got["view_once"], "unopened");
    assert_eq!(got["kind"], "voice");
    assert!(got["attachments"][0]["local_path"].is_null(), "{got}");
    // The sender keeps no copy once it is sent.
    alice.wait_msg("voice sent", &id, |m| m["status"] != "pending").await;
    let i2 = id.clone();
    poll("sender voice copy erased", || {
        let i2 = i2.clone();
        let core = alice.core.clone();
        async move {
            core.run(move |i| async move { Ok(i.message_row(&i2)?.unwrap().attachments[0].local_path.is_none()) }).await.unwrap()
        }
    })
    .await;

    // No forwarding, no saving.
    let (i3, c3) = (id.clone(), got["conversation_id"].as_str().unwrap().to_string());
    assert!(bob.core.run(move |i| async move { i.forward_messages(&[i3], &[c3]) }).await.is_err());
    let (i3, a3) = (id.clone(), got["attachments"][0]["id"].as_str().unwrap().to_string());
    assert!(bob.core.run(move |i| async move { i.save_attachment(&i3, &a3, "/tmp/should-not-exist.wav").await }).await.is_err());

    // Played once: the audio is there, a second open fails, then the file is gone.
    let i4 = id.clone();
    let att = bob.core.run(move |i| async move { i.open_view_once(&i4).await }).await.unwrap();
    assert_eq!(att.mime, "audio/wav");
    let path = att.local_path.clone().expect("player gets the file");
    let p = path.clone();
    let bytes = bob.core.run(move |i| async move { i.read_media_file(std::path::Path::new(&p)).await }).await.unwrap();
    assert_eq!(bytes, audio);
    let i4 = id.clone();
    assert!(bob.core.run(move |i| async move { i.open_view_once(&i4).await }).await.is_err(), "only once");
    let i4 = id.clone();
    bob.core.run(move |i| async move { i.view_once_done(&i4) }).await.unwrap();
    assert!(!std::path::Path::new(&path).exists(), "voice file deleted after playback");
    let i4 = id.clone();
    let row = bob.core.run(move |i| async move { Ok(i.message_row(&i4)?.unwrap()) }).await.unwrap();
    assert!(row.attachments[0].local_path.is_none(), "path forgotten");
    assert!(row.attachments[0].key.is_none() && row.attachments[0].cdn_id.is_none(), "key forgotten");
    let i4 = id.clone();
    assert!(bob.core.run(move |i| async move { i.open_view_once(&i4).await }).await.is_err(), "not after done either");

    // Bob's other device and the sender learn it was played.
    bob2.wait_msg("voice opened on bob2", &id, |m| m["view_once"] == "opened").await;
    let i5 = id.clone();
    let row2 = bob2.core.run(move |i| async move { Ok(i.message_row(&i5)?.unwrap()) }).await.unwrap();
    assert!(row2.attachments[0].local_path.is_none() && row2.attachments[0].key.is_none(), "erased on bob2");
    alice.wait_msg("voice opened for sender", &id, |m| m["view_once"] == "opened").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn profile_key_rotates_on_block() {
    if server().is_empty() {
        return;
    }
    let _ = env_logger::builder().is_test(true).try_init();
    let (alice, bob, carol) = (Client::new("alice"), Client::new("bob"), Client::new("carol"));
    let a = alice.register().await;
    let b = bob.register().await;
    let c = carol.register().await;
    let alice2 = Client::new("alice2");
    alice2.login(&a.number).await;
    alice.set_name("Alice").await;

    // Bob and Carol are Alice's contacts and have her profile key.
    let ab = alice.chat_with(&b.number).await;
    let acn = alice.chat_with(&c.number).await;
    bob.chat_with(&a.number).await;
    carol.chat_with(&a.number).await;
    alice.send(&ab.id, serde_json::json!({ "text": "hi bob" })).await.unwrap();
    alice.send(&acn.id, serde_json::json!({ "text": "hi carol" })).await.unwrap();
    let old_key = alice.profile_key().await;
    for (who, name) in [(&bob, "bob"), (&carol, "carol")] {
        let aid = a.account_id.clone();
        poll(&format!("{name} has alice's key"), || {
            let aid = aid.clone();
            async move { who.stored_contact_key(&aid).await.as_deref() == Some(&old_key[..]) }
        })
        .await;
    }
    let old_access = chat_core::crypto::unidentified_access_key(&old_key);
    let aid = a.account_id.clone();
    bob.core
        .run(move |i| async move { Ok(i.session()?.api.get_sealed_keys(&aid, "*", &old_access).await.is_ok()) })
        .await
        .map(|ok| assert!(ok, "old access key works before the block"))
        .unwrap();

    // Alice blocks Bob: the key rotates.
    let bid = b.account_id.clone();
    alice.core.run(move |i| async move { i.set_blocked(&bid, true) }).await.unwrap();
    poll("rotation", || async { alice.profile_key().await != old_key }).await;
    let new_key = alice.profile_key().await;

    // Carol and Alice's other device get the new key, Bob does not.
    let aid = a.account_id.clone();
    let cr = &carol;
    poll("carol gets the new key", || {
        let aid = aid.clone();
        async move { cr.stored_contact_key(&aid).await.as_deref() == Some(&new_key[..]) }
    })
    .await;
    poll("alice2 gets the new key", || async { alice2.profile_key().await == new_key }).await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(bob.stored_contact_key(&a.account_id).await.as_deref(), Some(&old_key[..]), "the blocked one keeps only the old key");

    // The published profile only opens with the new key.
    let aid = a.account_id.clone();
    carol.core.run(move |i| async move { i.fetch_profile(&aid).await }).await.unwrap();
    let aid = a.account_id.clone();
    let name = carol.core.run(move |i| async move { i.contact_model(&aid) }).await.unwrap().profile_name;
    assert_eq!(name.as_deref(), Some("Alice"));
    let aid = a.account_id.clone();
    assert!(bob.core.run(move |i| async move { i.fetch_profile(&aid).await }).await.is_err(), "old key cannot decrypt the profile");

    // The old access key (sealed sender, anonymous key fetches, presence) no longer works.
    let aid = a.account_id.clone();
    poll("old access key refused", || {
        let aid = aid.clone();
        let core = bob.core.clone();
        async move {
            core.run(move |i| async move { Ok(i.session()?.api.get_sealed_keys(&aid, "*", &old_access).await.is_err()) })
                .await
                .unwrap()
        }
    })
    .await;
    let new_access = chat_core::crypto::unidentified_access_key(&new_key);
    let aid = a.account_id.clone();
    let ok = carol
        .core
        .run(move |i| async move { Ok(i.session()?.api.get_sealed_keys(&aid, "*", &new_access).await.is_ok()) })
        .await
        .unwrap();
    assert!(ok, "the new access key works");
}
