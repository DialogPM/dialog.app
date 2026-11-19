//! Fills a local test server with realistic conversations for store screenshots: a main
//! account plus friends, direct chats (text, photo, voice note, reactions) and groups. The
//! main account is then opened in the app (log in with its number and password): its history
//! arrives through the encrypted vault.
//!
//! `cargo run -p chat-core --example seed_shots -- http://127.0.0.1:8081 <main password> <out dir>`
//! Prints the main account number. Test servers only.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
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

fn req<T: serde::de::DeserializeOwned>(v: Value) -> T {
    serde_json::from_value(v).expect("valid request")
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// A soft two-colour gradient (avatars, photos).
fn gradient(w: u32, h: u32, a: [u8; 3], b: [u8; 3], sun: bool) -> String {
    let img = image::RgbImage::from_fn(w, h, |x, y| {
        let t = (x as f32 / w as f32 * 0.4 + y as f32 / h as f32 * 0.6).min(1.0);
        let mut px = [0u8; 3];
        for i in 0..3 {
            px[i] = (a[i] as f32 * (1.0 - t) + b[i] as f32 * t) as u8;
        }
        if sun {
            let (dx, dy) = (x as f32 - w as f32 * 0.68, y as f32 - h as f32 * 0.3);
            if dx * dx + dy * dy < (h as f32 * 0.09).powi(2) {
                px = [250, 252, 240];
            }
            // Hills.
            let hill = h as f32 * (0.62 + 0.08 * ((x as f32 / w as f32) * 9.0).sin());
            if y as f32 > hill {
                px = [86, 140, 76];
            }
        }
        image::Rgb(px)
    });
    let mut png = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
    b64(&png)
}

fn voice_note(seconds: u32) -> (String, Vec<f32>) {
    let rate = 16_000u32;
    let samples: Vec<i16> = (0..rate * seconds)
        .map(|n| {
            let t = n as f32 / rate as f32;
            ((t * 220.0 * std::f32::consts::TAU).sin() * 6000.0 * (t * 2.3).sin().abs()) as i16
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
    let waveform = (0..48).map(|i| 0.15 + 0.85 * ((i as f32 / 48.0) * 7.0).sin().abs()).collect();
    (b64(&wav), waveform)
}

struct User {
    core: Core,
    account_id: String,
    number: String,
}

async fn user(server: &str, dir: &std::path::Path, name: &str, password: &str, avatar: Option<String>) -> User {
    std::fs::create_dir_all(dir).unwrap();
    let key = [7u8; 32];
    std::fs::write(dir.join("db.key"), key.iter().map(|b| format!("{b:02x}")).collect::<String>()).unwrap();
    let core = Core::start(CoreConfig {
        data_dir: dir.to_path_buf(),
        db_key: key,
        default_server_url: server.to_string(),
        platform: "cli".into(),
        events: Arc::new(Events::default()),
    })
    .unwrap();
    let s = server.to_string();
    let pw = password.to_string();
    let reg = core.run(move |i| async move { i.register(&s, &pw, "seed").await }).await.unwrap();
    core.run(|i| async move { i.confirm_mnemonic_saved() }).await.unwrap();
    let n = name.to_string();
    core.run(move |i| async move {
        i.set_profile(SetProfileRequest { name: Some(n), about: Some("Hey there! I am using Dialog.".into()), avatar_path: None, avatar_data_base64: avatar }).await
    })
    .await
    .unwrap();
    User { core, account_id: reg.account.account_id, number: reg.account.number }
}

impl User {
    async fn add(&self, other: &User) {
        let n = other.number.clone();
        self.core.run(move |i| async move { i.add_contact_by_number(&n, None).await }).await.unwrap();
    }

    async fn direct(&self, other: &User) -> String {
        let id = other.account_id.clone();
        self.core.run(move |i| async move { i.open_direct_conversation(&id).await }).await.unwrap().id
    }

    async fn send(&self, conv: &str, body: Value) -> Message {
        let mut body = body;
        body["conversation_id"] = json!(conv);
        let r: SendMessageRequest = req(body);
        let m = self.core.run(move |i| async move { i.send_message(r).await }).await.unwrap();
        tokio::time::sleep(Duration::from_millis(700)).await;
        m
    }

    async fn text(&self, conv: &str, t: &str) -> Message {
        self.send(conv, json!({ "text": t })).await
    }

    /// The conversation with `peer` as this user sees it (once delivered), accepted.
    async fn accept_from(&self, peer: &User) -> String {
        let conv = self.direct(peer).await;
        let c = conv.clone();
        let _ = self.core.run(move |i| async move { i.accept_conversation(&c).await }).await;
        conv
    }

    async fn group_conv(&self, name: &str) -> String {
        for _ in 0..40 {
            let n = name.to_string();
            let found = self
                .core
                .run(move |i| async move { Ok::<_, chat_core::CoreError>(i.list_conversations()?.into_iter().find(|c| c.title == n).map(|c| c.id)) })
                .await
                .unwrap();
            if let Some(id) = found {
                let c = id.clone();
                let _ = self.core.run(move |i| async move { i.accept_conversation(&c).await }).await;
                return id;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        panic!("group {name} did not arrive");
    }
}

/// Reopens an account created by an earlier run (same data dir and key).
fn reopen(server: &str, dir: &std::path::Path) -> Core {
    Core::start(CoreConfig {
        data_dir: dir.to_path_buf(),
        db_key: [7u8; 32],
        default_server_url: server.to_string(),
        platform: "cli".into(),
        events: Arc::new(Events::default()),
    })
    .unwrap()
}

/// Sends `text` from the friend in `dir` to the conversation titled `title` (or, with None,
/// its direct chat), so devices added since (the app being photographed) get its profile key.
async fn nudge(server: &str, dir: &std::path::Path, title: Option<&str>, text: &str) {
    let core = reopen(server, dir);
    // The local data of accounts with a password is locked until unlocked with it.
    core.run(|i| async move { i.unlock("Seed peer password 123").await }).await.unwrap();
    tokio::time::sleep(Duration::from_secs(4)).await;
    let all = core
        .run(|i| async move { Ok::<_, chat_core::CoreError>(i.list_conversations()?.into_iter().map(|c| (c.kind, c.title)).collect::<Vec<_>>()) })
        .await;
    eprintln!("{}: {all:?}", dir.display());
    let t = title.map(str::to_string);
    let conv = core
        .run(move |i| async move {
            Ok::<_, chat_core::CoreError>(
                i.list_conversations()?
                    .into_iter()
                    .find(|c| match &t {
                        Some(t) => &c.title == t,
                        None => c.kind == "direct",
                    })
                    .map(|c| c.id),
            )
        })
        .await
        .unwrap()
        .expect("conversation");
    let r: SendMessageRequest = req(json!({ "conversation_id": conv, "text": text }));
    core.run(move |i| async move { i.send_message(r).await }).await.unwrap();
    tokio::time::sleep(Duration::from_secs(3)).await;
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(2).map(String::as_str) == Some("--nudge") {
        let (server, out) = (args[1].clone(), std::path::PathBuf::from(&args[3]));
        let d = |n: &str| out.join(n);
        nudge(&server, &d("sarah"), None, "Ping me when you've read them").await;
        nudge(&server, &d("luca"), None, "I'll bring the printed copies").await;
        nudge(&server, &d("andrei"), None, "Perfect, see you at 10 ☕️").await;
        nudge(&server, &d("andrei"), Some("Project Aurora"), "Demo build is up on staging").await;
        nudge(&server, &d("elena"), Some("Weekend hiking 🥾"), "Count me in for the car!").await;
        nudge(&server, &d("mom"), Some("Family ❤️"), "Perfect, see you Sunday 💛").await;
        nudge(&server, &d("maria"), None, "And the headlamp!").await;
        return;
    }
    let server = args.get(1).expect("server url").clone();
    let main_password = args.get(2).expect("main account password").clone();
    let out = std::path::PathBuf::from(args.get(3).expect("data dir"));
    let peer_pw = "Seed peer password 123";
    let av = |a: [u8; 3], b: [u8; 3]| Some(gradient(256, 256, a, b, false));

    let me = user(&server, &out.join("me"), "Alex Morgan", &main_password, av([94, 196, 160], [30, 110, 150])).await;
    let maria = user(&server, &out.join("maria"), "Maria Popescu", peer_pw, av([120, 200, 240], [70, 130, 220])).await;
    let elena = user(&server, &out.join("elena"), "Elena Dumitru", peer_pw, av([240, 170, 120], [200, 90, 120])).await;
    let andrei = user(&server, &out.join("andrei"), "Andrei Ionescu", peer_pw, None).await;
    let luca = user(&server, &out.join("luca"), "Luca Rossi", peer_pw, av([140, 160, 250], [90, 80, 200])).await;
    let sarah = user(&server, &out.join("sarah"), "Sarah Chen", peer_pw, None).await;
    let mom = user(&server, &out.join("mom"), "Mom", peer_pw, av([250, 200, 150], [220, 120, 90])).await;
    println!("main account number: {}", me.number);

    for p in [&maria, &elena, &andrei, &luca, &sarah, &mom] {
        me.add(p).await;
        p.add(&me).await;
    }
    for (a, b) in [(&maria, &elena), (&elena, &andrei), (&andrei, &maria), (&mom, &maria)] {
        a.add(b).await;
        b.add(a).await;
    }

    // Direct chats: we write first (accepted on our side), they accept and answer.
    let (voice, wave) = voice_note(4);
    let photo = gradient(1200, 800, [170, 250, 210], [230, 250, 200], true);

    let c = me.direct(&sarah).await;
    me.text(&c, "Hi Sarah! Did you get a chance to look at the draft?").await;
    let cs = sarah.accept_from(&me).await;
    sarah.text(&cs, "Done, left a few comments").await;

    let c = me.direct(&luca).await;
    me.text(&c, "Sending you the budget sheet now").await;
    me.send(&c, json!({ "attachments": [{ "data_base64": b64(b"Category,Q1,Q2\nTravel,1200,950\nTools,400,620\n"), "file_name": "Budget_2026.xlsx", "mime": "text/csv", "kind": "file" }] })).await;
    let cl = luca.accept_from(&me).await;
    luca.text(&cl, "Thanks! Are you coming tomorrow?").await;

    let c = me.direct(&andrei).await;
    me.text(&c, "Ne vedem mâine la 10?").await;
    let ca = andrei.accept_from(&me).await;
    andrei.text(&ca, "Bine, mulțumesc! Ne vedem mâine?").await;

    // Groups.
    let ids = |us: &[&User]| us.iter().map(|u| u.account_id.clone()).collect::<Vec<_>>();
    for (name, members) in [
        ("Project Aurora", ids(&[&andrei, &elena, &sarah])),
        ("Weekend hiking 🥾", ids(&[&maria, &elena, &andrei])),
        ("Family ❤️", ids(&[&mom, &maria])),
    ] {
        let r: CreateGroupRequest = req(json!({ "name": name, "member_account_ids": members }));
        me.core.run(move |i| async move { i.create_group(r).await }).await.unwrap();
    }
    tokio::time::sleep(Duration::from_secs(3)).await;

    let g = me.group_conv("Project Aurora").await;
    me.text(&g, "Can we move the demo to Thursday?").await;
    let ga = andrei.group_conv("Project Aurora").await;
    andrei.text(&ga, "Thursday works for backend, the API is ready").await;

    let g = me.group_conv("Weekend hiking 🥾").await;
    me.text(&g, "Who's driving on Saturday?").await;
    let gm = maria.group_conv("Weekend hiking 🥾").await;
    maria.text(&gm, "I can take 3 people 🚗").await;
    let ge = elena.group_conv("Weekend hiking 🥾").await;
    elena.send(&ge, json!({ "attachments": [{ "data_base64": voice.clone(), "file_name": "voice.wav", "mime": "audio/wav", "kind": "voice", "duration_ms": 4000, "waveform": wave.clone() }] })).await;

    let g = me.group_conv("Family ❤️").await;
    let gmom = mom.group_conv("Family ❤️").await;
    mom.text(&gmom, "Dinner on Sunday at 6?").await;
    me.text(&g, "Yes! I'll bring dessert 🍰").await;

    // Maria last, so she is on top of the list.
    let c = me.direct(&maria).await;
    me.text(&c, "I'll leave around 7 so we avoid traffic").await;
    me.send(&c, json!({ "attachments": [{ "data_base64": voice, "file_name": "voice.wav", "mime": "audio/wav", "kind": "voice", "duration_ms": 4000, "waveform": wave }] })).await;
    let cm = maria.accept_from(&me).await;
    maria.send(&cm, json!({ "text": "Forwarding this from the hiking group", "attachments": [{ "data_base64": photo, "file_name": "mountains.png", "mime": "image/png", "kind": "image" }] })).await;
    maria.text(&cm, "Also: don't forget warm clothes, it's going to be −5°C at night 🥶").await;
    maria.text(&cm, "And the headlamp!").await;

    // Time for the vault uploads of the main account and the deliveries.
    tokio::time::sleep(Duration::from_secs(15)).await;
    println!("main account number: {}", me.number);
}
