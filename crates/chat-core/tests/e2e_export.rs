//! Exporting a conversation (engine/export.rs) against a real server: a direct chat, a group
//! and a chat with a bot, as plain text and as an archive with the media. The export holds
//! the transcript the UI wrote and only the media the conversation still shows.
//!
//! `CHATAPP_E2E_SERVER=http://127.0.0.1:8081 cargo test -p chat-core --test e2e_export`

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use chat_core::{Core, CoreConfig, CoreError, ErrorCode, EventSink};
use serde_json::{json, Value};

struct NoEvents;

impl EventSink for NoEvents {
    fn emit(&self, _event: &str, _payload: Value) {}
}

fn server() -> String {
    std::env::var("CHATAPP_E2E_SERVER").unwrap_or_default()
}

const PASSWORD: &str = "Export-test-password-1";

struct Client(Core);

impl Client {
    fn new(dir: &Path) -> Self {
        std::fs::create_dir_all(dir).unwrap();
        Self(
            Core::start(CoreConfig {
                data_dir: dir.to_path_buf(),
                db_key: [7u8; 32],
                default_server_url: server(),
                platform: "cli".into(),
                events: Arc::new(NoEvents),
            })
            .unwrap(),
        )
    }

    /// A command as the UIs call it (rpc.rs).
    async fn try_rpc(&self, cmd: &str, req: Value) -> Result<Value, CoreError> {
        chat_core::rpc::dispatch(&self.0, cmd, req).await
    }

    async fn rpc(&self, cmd: &str, req: Value) -> Value {
        self.try_rpc(cmd, req).await.unwrap_or_else(|e| panic!("{cmd}: {e:?}"))
    }

    async fn register(&self, name: &str) -> Value {
        self.rpc("register", json!({"server_url": server(), "password": PASSWORD, "device_name": name})).await["account"].clone()
    }

    async fn send(&self, conv: &str, mut msg: Value) -> Value {
        msg["conversation_id"] = json!(conv);
        self.rpc("send_message", msg).await
    }

    /// Waits for a message with this text in `conv`.
    async fn wait_text(&self, conv: &str, text: &str) -> Value {
        for _ in 0..150 {
            let all = self.rpc("get_messages", json!({"conversation_id": conv, "limit": 200})).await;
            if let Some(m) = all.as_array().unwrap().iter().find(|m| m["text"] == text) {
                return m.clone();
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("timed out waiting for '{text}'");
    }

    /// Waits for the direct conversation with this account.
    async fn wait_conversation_with(&self, account_id: &Value) -> Value {
        for _ in 0..150 {
            let all = self.rpc("list_conversations", json!({})).await;
            if let Some(c) = all.as_array().unwrap().iter().find(|c| &c["peer_account_id"] == account_id) {
                return c.clone();
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("timed out waiting for the conversation with {account_id}");
    }
}

fn b64(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}

fn png(seed: u8) -> Vec<u8> {
    let mut out = Vec::new();
    image::RgbImage::from_fn(48, 32, |x, y| image::Rgb([x as u8 * 5, y as u8 * 7, seed]))
        .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .unwrap();
    out
}

fn photo(file_name: &str, data: &[u8]) -> Value {
    json!({ "data_base64": b64(data), "file_name": file_name, "mime": "image/png", "kind": "image" })
}

fn media(message: &Value, index: usize, name: &str) -> Value {
    json!({ "message_id": message["id"], "attachment_id": message["attachments"][index]["id"], "name": name })
}

/// The files of a ZIP archive, read from its central directory (every entry is stored).
fn unzip(path: &Path) -> Vec<(String, Vec<u8>)> {
    let b = std::fs::read(path).unwrap();
    let u16_at = |i: usize| u16::from_le_bytes([b[i], b[i + 1]]) as usize;
    let u32_at = |i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]) as usize;
    let end = b.len() - 22;
    assert_eq!(u32_at(end), 0x0605_4b50, "not a ZIP archive");
    let (count, mut at) = (u16_at(end + 10), u32_at(end + 16));
    let mut out = Vec::new();
    for _ in 0..count {
        assert_eq!(u32_at(at), 0x0201_4b50);
        assert_eq!(u16_at(at + 10), 0, "stored");
        let (len, name_len, local) = (u32_at(at + 24), u16_at(at + 28), u32_at(at + 42));
        let name = String::from_utf8(b[at + 46..at + 46 + name_len].to_vec()).unwrap();
        let data = local + 30 + u16_at(local + 26) + u16_at(local + 28);
        out.push((name, b[data..data + len].to_vec()));
        at += 46 + name_len;
    }
    // The system's own unpacker agrees, where there is one.
    if let Ok(o) = std::process::Command::new("unzip").arg("-tqq").arg(path).output() {
        assert!(o.status.success(), "unzip -t: {}", String::from_utf8_lossy(&o.stdout));
    }
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn conversations_are_exported_as_text_and_as_an_archive() {
    if server().is_empty() {
        eprintln!("CHATAPP_E2E_SERVER not set, skipping");
        return;
    }
    let _ = env_logger::builder().try_init();
    let base = tempfile::tempdir().unwrap();
    let out = base.path().join("out");
    std::fs::create_dir_all(&out).unwrap();
    let dest = |name: &str| out.join(name).to_string_lossy().to_string();

    let ana = Client::new(&base.path().join("ana"));
    let bob = Client::new(&base.path().join("bob"));
    let ana_acc = ana.register("ana").await;
    let bob_acc = bob.register("bob").await;

    // --- a direct chat -------------------------------------------------------------------
    let contact = ana.rpc("add_contact_by_number", json!({"number": bob_acc["number"]})).await;
    let conv = ana.rpc("open_direct_conversation", json!({"account_id": contact["account_id"]})).await;
    let conv_id = conv["id"].as_str().unwrap().to_string();
    ana.send(&conv_id, json!({"text": "hello bob"})).await;
    let (pic, notes) = (png(1), b"line one\nline two\n".to_vec());
    let with_files = ana
        .send(
            &conv_id,
            json!({"text": "two files", "attachments": [
                photo("pic.png", &pic),
                { "data_base64": b64(&notes), "file_name": "notes.txt", "mime": "text/plain", "kind": "file" }
            ]}),
        )
        .await;
    let secret = ana.send(&conv_id, json!({"view_once": true, "attachments": [photo("secret.png", &png(2))]})).await;
    let gone = ana.send(&conv_id, json!({"text": "oops", "attachments": [photo("oops.png", &png(3))]})).await;
    // Bob answers, so the chat has both sides.
    let bob_conv = bob.wait_conversation_with(&ana_acc["account_id"]).await;
    let bob_conv_id = bob_conv["id"].as_str().unwrap().to_string();
    bob.rpc("accept_conversation", json!({"conversation_id": bob_conv_id})).await;
    bob.send(&bob_conv_id, json!({"text": "hello ana"})).await;
    ana.wait_text(&conv_id, "hello ana").await;
    ana.rpc("delete_message", json!({"message_id": gone["id"], "for_everyone": false})).await;

    // Text only: the file is the transcript the UI wrote, byte for byte.
    let transcript = "Chat with Bob\n\n[2026-11-28 14:03] You: hello bob\n[2026-11-28 14:04] Bob: hello ana 👋\n";
    let txt = dest("direct.txt");
    let r = ana.rpc("export_conversation", json!({"conversation_id": conv_id, "dest_path": txt, "text": transcript})).await;
    assert_eq!(r, json!({"media_exported": 0, "media_skipped": 0}));
    assert_eq!(std::fs::read_to_string(&txt).unwrap(), transcript);

    // With media: an archive with the transcript and the files, decrypted. View-once media,
    // the media of a deleted message and unknown messages are left out.
    let zip = dest("direct.zip");
    let r = ana
        .rpc(
            "export_conversation",
            json!({"conversation_id": conv_id, "dest_path": zip, "text": transcript, "archive": true, "text_name": "Conversație.txt",
                "media": [
                    media(&with_files, 0, "0001-pic.png"),
                    media(&with_files, 1, "0002-notes.txt"),
                    media(&secret, 0, "0003-secret.png"),
                    media(&gone, 0, "0004-oops.png"),
                    { "message_id": "no-such-message", "attachment_id": "x", "name": "0005-nothing.bin" }
                ]}),
        )
        .await;
    assert_eq!(r, json!({"media_exported": 2, "media_skipped": 3}));
    let files = unzip(Path::new(&zip));
    assert_eq!(
        files,
        vec![
            ("Conversație.txt".to_string(), transcript.as_bytes().to_vec()),
            ("media/0001-pic.png".to_string(), pic.clone()),
            ("media/0002-notes.txt".to_string(), notes.clone()),
        ]
    );
    // The app's own copies stay encrypted.
    let local = with_files["attachments"][0]["local_path"].as_str().unwrap();
    assert_ne!(std::fs::read(local).unwrap(), pic);

    // The receiving side exports what it downloaded.
    let got = bob.wait_text(&bob_conv_id, "two files").await;
    for a in got["attachments"].as_array().unwrap() {
        if a["download_state"] != "done" {
            bob.rpc("download_attachment", json!({"message_id": got["id"], "attachment_id": a["id"]})).await;
        }
    }
    let got = bob.wait_text(&bob_conv_id, "two files").await;
    assert!(got["attachments"].as_array().unwrap().iter().all(|a| a["download_state"] == "done"), "{got}");
    let bob_zip = dest("bob.zip");
    let r = bob
        .rpc(
            "export_conversation",
            json!({"conversation_id": bob_conv_id, "dest_path": bob_zip, "text": transcript, "archive": true,
                "media": [media(&got, 0, "a.png"), media(&got, 1, "b.txt")]}),
        )
        .await;
    assert_eq!(r["media_exported"], 2);
    let files = unzip(Path::new(&bob_zip));
    assert_eq!(files[0].0, "chat.txt", "the default name of the transcript");
    assert_eq!((files[1].1.clone(), files[2].1.clone()), (pic.clone(), notes.clone()));

    // --- a group -------------------------------------------------------------------------
    let group = ana
        .rpc("create_group", json!({"name": "Team", "description": null, "member_account_ids": [contact["account_id"]], "avatar_path": null}))
        .await;
    let group_id = group["id"].as_str().unwrap().to_string();
    ana.send(&group_id, json!({"text": "hello team"})).await;
    let group_pic = png(4);
    let in_group = ana.send(&group_id, json!({"attachments": [photo("team.png", &group_pic)]})).await;
    let group_zip = dest("group.zip");
    let r = ana
        .rpc(
            "export_conversation",
            json!({"conversation_id": group_id, "dest_path": group_zip, "text": "Team\n", "archive": true, "text_name": "Team.txt",
                "media": [media(&in_group, 0, "0001-team.png")]}),
        )
        .await;
    assert_eq!(r, json!({"media_exported": 1, "media_skipped": 0}));
    assert_eq!(
        unzip(Path::new(&group_zip)),
        vec![("Team.txt".to_string(), b"Team\n".to_vec()), ("media/0001-team.png".to_string(), group_pic)]
    );

    // Media of another conversation is refused, and no half-written archive is left behind.
    let stray = dest("stray.zip");
    let err = ana
        .try_rpc(
            "export_conversation",
            json!({"conversation_id": group_id, "dest_path": stray, "text": "x", "archive": true, "media": [media(&with_files, 0, "pic.png")]}),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidInput);
    assert!(!Path::new(&stray).exists());
    // Names that would leave the archive's folder, and two files under one name, are refused.
    for names in [vec!["../pic.png"], vec!["a/b.png"], vec![""], vec!["same.png", "SAME.png"]] {
        let items: Vec<Value> = names.iter().map(|n| media(&with_files, 0, n)).collect();
        let err = ana
            .try_rpc("export_conversation", json!({"conversation_id": conv_id, "dest_path": stray, "text": "x", "archive": true, "media": items}))
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput, "{names:?}");
        assert!(!Path::new(&stray).exists());
    }
    let err = ana
        .try_rpc("export_conversation", json!({"conversation_id": conv_id, "dest_path": stray, "text": "x", "archive": true, "text_name": "../chat.txt"}))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidInput);
    // Media without an archive has nowhere to go.
    let err = ana
        .try_rpc("export_conversation", json!({"conversation_id": conv_id, "dest_path": stray, "text": "x", "media": [media(&with_files, 0, "pic.png")]}))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidInput);
    let err = ana.try_rpc("export_conversation", json!({"conversation_id": "no-such-chat", "dest_path": stray, "text": "x"})).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
    assert!(!Path::new(&stray).exists());

    // --- a chat with a bot ---------------------------------------------------------------
    let username = format!("x{}_bot", &ana_acc["account_id"].as_str().unwrap()[..8]);
    let created = ana.rpc("bot_create", json!({"username": username, "name": "Weather", "about": ""})).await;
    let (bot_id, token) = (created["bot"]["id"].clone(), created["token"].as_str().unwrap().to_string());
    let bot_conv = bob.rpc("bot_open", json!({"bot_id": bot_id})).await;
    let bot_conv_id = bot_conv["id"].as_str().unwrap().to_string();
    bob.rpc("bot_start", json!({"bot_id": bot_id})).await;
    let http = reqwest::Client::new();
    let ups: Value = http
        .post(format!("{}/bot/{token}/getUpdates", server()))
        .json(&json!({"offset": 0, "timeout": 5}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let chat = ups["result"][0]["message"]["chat"]["id"].clone();
    let sent = http.post(format!("{}/bot/{token}/sendMessage", server())).json(&json!({"chat_id": chat, "text": "Rain at 5 pm"})).send().await.unwrap();
    assert_eq!(sent.status().as_u16(), 200);
    bob.wait_text(&bot_conv_id, "Rain at 5 pm").await;
    let bot_transcript = "Chat with Weather\n\n[2026-11-28 14:03] Weather: Rain at 5 pm\n";
    let bot_txt = dest("bot.txt");
    bob.rpc("export_conversation", json!({"conversation_id": bot_conv_id, "dest_path": bot_txt, "text": bot_transcript})).await;
    assert_eq!(std::fs::read_to_string(&bot_txt).unwrap(), bot_transcript);
    let bot_zip = dest("bot.zip");
    let r = bob
        .rpc("export_conversation", json!({"conversation_id": bot_conv_id, "dest_path": bot_zip, "text": bot_transcript, "archive": true}))
        .await;
    assert_eq!(r, json!({"media_exported": 0, "media_skipped": 0}));
    assert_eq!(unzip(Path::new(&bot_zip)), vec![("chat.txt".to_string(), bot_transcript.as_bytes().to_vec())]);
}
