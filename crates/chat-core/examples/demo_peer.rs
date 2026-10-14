//! Headless demo peer: registers an account, messages a given account number and answers
//! incoming messages for a while. Useful to try the app with a second user.
//!
//! `cargo run -p chat-core --example demo_peer -- http://127.0.0.1:8080 <number> [name]`

use std::sync::{Arc, Mutex};
use std::time::Duration;

use chat_core::model::*;
use chat_core::{Core, CoreConfig, EventSink};
use serde_json::{json, Value};

#[derive(Default)]
struct Events(Mutex<Vec<(String, Value)>>);

impl EventSink for Events {
    fn emit(&self, event: &str, payload: Value) {
        self.0.lock().unwrap().push((event.to_string(), payload));
    }
}

fn hex_key(k: &[u8]) -> String {
    k.iter().map(|b| format!("{b:02x}")).collect()
}

fn req<T: serde::de::DeserializeOwned>(v: Value) -> T {
    serde_json::from_value(v).expect("valid request")
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let server = args.get(1).expect("server url").clone();
    let number = args.get(2).expect("account number to message").clone();
    let name = args.get(3).cloned().unwrap_or_else(|| "Maria Popescu".into());
    // DEMO_DATA_DIR keeps the account: the desktop app can then open it as a second user
    // (`CHATAPP_DATA_DIR=<dir> chatapp`), since db.key holds the same key the core uses here.
    let tmp = tempfile::tempdir().unwrap();
    let data_dir = match std::env::var_os("DEMO_DATA_DIR") {
        Some(d) => {
            let d = std::path::PathBuf::from(d);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("db.key"), hex_key(&[1u8; 32])).unwrap();
            d
        }
        None => tmp.path().to_path_buf(),
    };
    let events = Arc::new(Events::default());
    let core = Core::start(CoreConfig {
        data_dir,
        db_key: [1u8; 32],
        default_server_url: server.clone(),
        platform: "cli".into(),
        events: events.clone(),
    })
    .unwrap();

    let s = server.clone();
    let reg = core.run(move |i| async move { i.register(&s, "Demo password 123", "demo peer").await }).await.unwrap();
    println!("registered {}", reg.account.number);
    core.run(|i| async move { i.confirm_mnemonic_saved() }).await.unwrap();
    let n = name.clone();
    core.run(move |i| async move {
        i.set_profile(SetProfileRequest { name: Some(n), about: Some("Hey there! I am using Dialog.".into()), avatar_path: None, avatar_data_base64: None }).await
    })
    .await
    .unwrap();
    let contact = core.run(move |i| async move { i.add_contact_by_number(&number, None).await }).await.unwrap();
    let conv = core
        .run(move |i| async move { i.open_direct_conversation(&contact.account_id).await })
        .await
        .unwrap();

    let send = |body: Value| {
        let core = core.clone();
        async move {
            let r: SendMessageRequest = req(body);
            core.run(move |i| async move { i.send_message(r).await }).await.unwrap()
        }
    };
    send(json!({ "conversation_id": conv.id, "text": "Salut! 👋 Am instalat și eu Dialog." })).await;
    tokio::time::sleep(Duration::from_millis(800)).await;
    send(json!({ "conversation_id": conv.id, "text": "Totul e criptat end-to-end cu protocolul Signal, nici serverul nu poate citi mesajele 🔒" })).await;

    // A generated picture.
    let mut png = Vec::new();
    image::RgbImage::from_fn(640, 400, |x, y| {
        let t = (x as f32 / 640.0, y as f32 / 400.0);
        image::Rgb([(20.0 + 60.0 * t.0) as u8, (140.0 + 80.0 * t.1) as u8, (170.0 - 40.0 * t.0) as u8])
    })
    .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
    .unwrap();
    use base64::Engine;
    let data = base64::engine::general_purpose::STANDARD.encode(&png);
    tokio::time::sleep(Duration::from_millis(800)).await;
    send(json!({ "conversation_id": conv.id, "text": "Uite o poză de la munte",
        "attachments": [{ "data_base64": data, "file_name": "munte.png", "mime": "image/png", "kind": "image" }] }))
    .await;
    tokio::time::sleep(Duration::from_millis(800)).await;
    send(json!({ "conversation_id": conv.id,
        "location": { "lat": 44.4268, "lng": 26.1025, "name": "Piața Universității", "address": "București" } }))
    .await;

    // A short voice note (a WAV tone), sent the way the UI recorder sends one.
    let rate = 16_000u32;
    let samples: Vec<i16> = (0..rate * 2)
        .map(|n| {
            let t = n as f32 / rate as f32;
            ((t * 440.0 * std::f32::consts::TAU).sin() * 8000.0 * (t * 3.0).sin().abs()) as i16
        })
        .collect();
    let mut wav = Vec::with_capacity(44 + samples.len() * 2);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + samples.len() as u32 * 2).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&rate.to_le_bytes());
    wav.extend_from_slice(&(rate * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(samples.len() as u32 * 2).to_le_bytes());
    for s in &samples {
        wav.extend_from_slice(&s.to_le_bytes());
    }
    let waveform: Vec<f32> = (0..48).map(|i| 0.1 + 0.9 * ((i as f32 / 48.0) * 6.0).sin().abs()).collect();
    tokio::time::sleep(Duration::from_millis(800)).await;
    send(json!({ "conversation_id": conv.id,
        "attachments": [{ "data_base64": base64::engine::general_purpose::STANDARD.encode(&wav), "file_name": "voice.wav",
            "mime": "audio/wav", "kind": "voice", "duration_ms": 2000, "waveform": waveform }] }))
    .await;

    // Answer incoming messages for a few minutes.
    let mut answered = std::collections::HashSet::new();
    for _ in 0..1800 {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let incoming: Vec<(String, String)> = events
            .0
            .lock()
            .unwrap()
            .iter()
            .filter(|(n, v)| n == "message" && v["is_new_incoming"] == true)
            .map(|(_, v)| {
                let m = &v["message"];
                let text = match m["text"].as_str() {
                    Some(t) if !t.is_empty() => t.to_string(),
                    _ => format!("[{}]", m["kind"].as_str().unwrap_or("?")),
                };
                (m["id"].as_str().unwrap_or_default().to_string(), text)
            })
            .collect();
        for (id, text) in incoming {
            if answered.insert(id.clone()) {
                let c = conv.id.clone();
                core.run(move |i| async move { i.mark_read(&c) }).await.unwrap();
                let rid = id.clone();
                core.run(move |i| async move { i.react(&rid, Some("❤️".into())) }).await.unwrap();
                tokio::time::sleep(Duration::from_millis(600)).await;
                send(json!({ "conversation_id": conv.id, "reply_to": id, "text": format!("Am primit: „{text}” 😊") })).await;
            }
        }
    }
}
