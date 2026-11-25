//! Bots against a real server: an account creates a bot and drives it through the Bot API;
//! another account finds it, starts it, talks to it and stops it. The bot can write only
//! between start and stop, and never learns who it is talking to.
//!
//! `CHATAPP_E2E_SERVER=http://127.0.0.1:8081 cargo test -p chat-core --test e2e_bots`

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use chat_core::model::{BotCommand, Conversation, Message, SendMessageRequest};
use chat_core::{Core, CoreConfig, ErrorCode, EventSink};
use serde_json::{json, Value};

struct NoEvents;

impl EventSink for NoEvents {
    fn emit(&self, _event: &str, _payload: Value) {}
}

fn server() -> String {
    std::env::var("CHATAPP_E2E_SERVER").unwrap_or_default()
}

fn device(dir: &Path) -> Core {
    std::fs::create_dir_all(dir).unwrap();
    Core::start(CoreConfig {
        data_dir: dir.to_path_buf(),
        db_key: [7u8; 32],
        default_server_url: server(),
        platform: "cli".into(),
        events: Arc::new(NoEvents),
    })
    .unwrap()
}

const PASSWORD: &str = "Bots-test-password-1";

/// One Bot API call: (HTTP status, body).
async fn bot_api(token: &str, method: &str, params: Value) -> (u16, Value) {
    let url = format!("{}/bot/{token}/{method}", server());
    let resp = reqwest::Client::new().post(url).json(&params).send().await.unwrap();
    let status = resp.status().as_u16();
    (status, resp.json().await.unwrap_or(Value::Null))
}

/// The updates waiting for the bot, confirming everything below `offset`.
async fn updates(token: &str, offset: i64) -> Vec<Value> {
    let (status, body) = bot_api(token, "getUpdates", json!({"offset": offset, "timeout": 5})).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["ok"], true);
    body["result"].as_array().cloned().unwrap()
}

async fn messages(core: &Core, conv: &str) -> Vec<Message> {
    let id = conv.to_string();
    core.run(move |i| async move { i.get_messages(&id, None, Some(200)) }).await.unwrap()
}

async fn conversation(core: &Core, conv: &str) -> Conversation {
    let id = conv.to_string();
    core.run(move |i| async move { i.get_conversation(&id) }).await.unwrap()
}

/// Waits until `check` holds for the messages of `conv`.
async fn wait_messages(core: &Core, conv: &str, what: &str, check: impl Fn(&[Message]) -> bool) -> Vec<Message> {
    for _ in 0..100 {
        let all = messages(core, conv).await;
        if check(&all) {
            return all;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("timed out waiting for {what}");
}

async fn send_text(core: &Core, conv: &str, text: &str) -> Result<Message, chat_core::CoreError> {
    let req: SendMessageRequest = serde_json::from_value(json!({"conversation_id": conv, "text": text})).unwrap();
    core.run(move |i| async move { i.send_message(req).await }).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_bot_from_creation_to_deletion() {
    if server().is_empty() {
        eprintln!("CHATAPP_E2E_SERVER not set, skipping");
        return;
    }
    let _ = env_logger::builder().try_init();
    let base = tempfile::tempdir().unwrap();
    let url = server();

    let owner = device(&base.path().join("owner"));
    let u = url.clone();
    let owner_acc = owner.run(move |i| async move { i.register(&u, PASSWORD, "owner").await }).await.unwrap().account;
    let user = device(&base.path().join("user"));
    let u = url.clone();
    let user_acc = user.run(move |i| async move { i.register(&u, PASSWORD, "user").await }).await.unwrap().account;

    // A bot's username ends in "bot"; anything else is refused.
    let err = owner.run(|i| async move { i.bot_create("weather", "Weather", "").await }).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidInput);
    let username = format!("w{}_bot", &owner_acc.account_id[..8]);
    let name = username.clone();
    let created = owner.run(move |i| async move { i.bot_create(&name, "Weather", "Rain alerts").await }).await.unwrap();
    assert_eq!(created.bot.username, username);
    let (bot_id, token) = (created.bot.id.clone(), created.token.clone());
    let name = username.clone();
    let err = owner.run(move |i| async move { i.bot_create(&name, "Again", "").await }).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::Conflict, "a username belongs to one bot");

    // The Bot API knows the bot by its token alone.
    let (status, me) = bot_api(&token, "getMe", json!({})).await;
    assert_eq!((status, me["result"]["username"].as_str()), (200, Some(username.as_str())));
    let (status, _) = bot_api("not-a-token", "getMe", json!({})).await;
    assert_eq!(status, 401);
    let (status, _) = bot_api(&token, "setMyCommands", json!({"commands": [{"command": "status", "description": "How things are"}]})).await;
    assert_eq!(status, 200);

    // Someone else finds it by username: nothing started yet, so the bot cannot reach them.
    let name = format!("@{username}");
    let found = user.run(move |i| async move { i.bot_find(&name).await }).await.unwrap();
    assert_eq!((found.id.as_str(), found.state.as_str(), found.mine), (bot_id.as_str(), "none", false));
    assert_eq!(found.commands, vec![BotCommand { command: "status".into(), description: "How things are".into() }]);
    // The bot has no way to name a person: not by account id, not by number.
    for chat in [user_acc.account_id.as_str(), user_acc.number.as_str(), "0123456789abcdef0123456789abcdef"] {
        let (status, body) = bot_api(&token, "sendMessage", json!({"chat_id": chat, "text": "spam"})).await;
        assert_eq!(status, 400, "{body}");
    }

    let id = bot_id.clone();
    let conv = user.run(move |i| async move { i.bot_open(&id).await }).await.unwrap();
    let bot = conv.bot.clone().expect("a bot conversation");
    assert_eq!((conv.title.as_str(), bot.state.as_str(), conv.accepted), ("Weather", "none", true));
    let err = send_text(&user, &conv.id, "hello?").await.unwrap_err();
    assert_eq!(err.code, ErrorCode::Forbidden, "nothing is sent to a bot that was not started");
    assert!(updates(&token, 0).await.is_empty());

    // Start: the bot gets "/start" and a chat id that says nothing about the account.
    let id = bot_id.clone();
    let started = user.run(move |i| async move { i.bot_start(&id).await }).await.unwrap();
    assert_eq!(started.bot.unwrap().state, "active");
    let ups = updates(&token, 0).await;
    assert_eq!(ups.len(), 1, "{ups:?}");
    assert_eq!(ups[0]["message"]["text"], "/start");
    let chat = ups[0]["message"]["chat"]["id"].as_str().unwrap().to_string();
    assert!(chat != user_acc.account_id && !chat.contains(&user_acc.number));
    let next = ups[0]["update_id"].as_i64().unwrap() + 1;
    let mine = messages(&user, &conv.id).await;
    assert!(mine.iter().any(|m| m.system.as_ref().is_some_and(|s| s.kind == "bot_started")));

    // Bot -> person.
    let (status, sent) = bot_api(&token, "sendMessage", json!({"chat_id": chat, "text": "Rain at 5 pm"})).await;
    assert_eq!(status, 200, "{sent}");
    let all = wait_messages(&user, &conv.id, "the bot's message", |m| m.iter().any(|m| m.text.as_deref() == Some("Rain at 5 pm"))).await;
    let got = all.iter().find(|m| m.text.as_deref() == Some("Rain at 5 pm")).unwrap();
    assert!(!got.outgoing);
    assert_eq!(got.sender_account_id, bot_id);
    assert_eq!(conversation(&user, &conv.id).await.unread_count, 1);

    // Person -> bot.
    let out = send_text(&user, &conv.id, "/status").await.unwrap();
    wait_messages(&user, &conv.id, "the message to the bot to be sent", |m| m.iter().any(|m| m.id == out.id && m.status == "sent")).await;
    let ups = updates(&token, next).await;
    assert_eq!(ups.len(), 1, "{ups:?}");
    assert_eq!((ups[0]["message"]["text"].as_str(), ups[0]["message"]["chat"]["id"].as_str()), (Some("/status"), Some(chat.as_str())));
    let next = ups[0]["update_id"].as_i64().unwrap() + 1;
    // Only text reaches a bot.
    let req: SendMessageRequest = serde_json::from_value(json!({
        "conversation_id": conv.id, "text": "here", "location": {"lat": 1.0, "lng": 2.0}
    }))
    .unwrap();
    let err = user.run(move |i| async move { i.send_message(req).await }).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidInput);

    // Stop: the bot is told, and its messages are refused from then on.
    let id = bot_id.clone();
    let stopped = user.run(move |i| async move { i.bot_stop(&id).await }).await.unwrap();
    assert_eq!(stopped.bot.unwrap().state, "stopped");
    let ups = updates(&token, next).await;
    assert_eq!(ups.len(), 1, "{ups:?}");
    assert_eq!(ups[0]["chat_stopped"]["chat"]["id"], chat.as_str());
    let (status, body) = bot_api(&token, "sendMessage", json!({"chat_id": chat, "text": "come back"})).await;
    assert_eq!((status, body["reason"].as_str()), (403, Some("bot_stopped")), "{body}");
    assert_eq!(send_text(&user, &conv.id, "anyone?").await.unwrap_err().code, ErrorCode::Forbidden);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!messages(&user, &conv.id).await.iter().any(|m| m.text.as_deref() == Some("come back")));

    // Starting again resumes the same chat.
    let id = bot_id.clone();
    user.run(move |i| async move { i.bot_start(&id).await }).await.unwrap();
    let (status, _) = bot_api(&token, "sendMessage", json!({"chat_id": chat, "text": "welcome back"})).await;
    assert_eq!(status, 200);
    wait_messages(&user, &conv.id, "the message after restart", |m| m.iter().any(|m| m.text.as_deref() == Some("welcome back"))).await;

    // The owner's side: the list, a new token, deletion.
    let bots = owner.run(|i| async move { i.bots_list().await }).await.unwrap();
    assert_eq!(bots.len(), 1);
    assert_eq!((bots[0].id.as_str(), bots[0].subscribers), (bot_id.as_str(), 1));
    assert_eq!(bots[0].token.as_deref(), Some(token.as_str()), "the device that created the bot can show its token again");
    // The owner sees the same chat id the bot sees, and can write to it without the token.
    let id = bot_id.clone();
    let subs = owner.run(move |i| async move { i.bot_subscribers(&id).await }).await.unwrap();
    assert_eq!(subs.len(), 1);
    assert_eq!((subs[0].chat_id.as_str(), subs[0].mine), (chat.as_str(), false));
    let (id, to) = (bot_id.clone(), chat.clone());
    owner.run(move |i| async move { i.bot_send_test(&id, &to, "test from settings").await }).await.unwrap();
    wait_messages(&user, &conv.id, "the owner's test message", |m| m.iter().any(|m| m.text.as_deref() == Some("test from settings"))).await;
    let id = bot_id.clone();
    assert!(user.run(move |i| async move { i.bot_subscribers(&id).await }).await.is_err());
    assert!(user.run(|i| async move { i.bots_list().await }).await.unwrap().is_empty());
    let id = bot_id.clone();
    let err = user.run(move |i| async move { i.bot_delete(&id).await }).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound, "only the owner manages a bot");
    let id = bot_id.clone();
    let renamed = owner.run(move |i| async move { i.bot_update(&id, Some("Weather 2".into()), None, None).await }).await.unwrap();
    assert_eq!(renamed.name, "Weather 2");
    let id = bot_id.clone();
    let new_token = owner.run(move |i| async move { i.bot_reset_token(&id).await }).await.unwrap();
    assert_eq!(bot_api(&token, "getMe", json!({})).await.0, 401, "the old token stops working");
    assert_eq!(bot_api(&new_token, "getMe", json!({})).await.0, 200);

    let id = bot_id.clone();
    owner.run(move |i| async move { i.bot_delete(&id).await }).await.unwrap();
    assert_eq!(bot_api(&new_token, "getMe", json!({})).await.0, 401);
    let mut gone = false;
    for _ in 0..100 {
        if conversation(&user, &conv.id).await.bot.is_some_and(|b| b.deleted) {
            gone = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(gone, "the people who talked to a deleted bot are told");
    assert_eq!(send_text(&user, &conv.id, "still there?").await.unwrap_err().code, ErrorCode::Forbidden);

    owner.shutdown().await;
    user.shutdown().await;
}

/// A 1x1 PNG.
const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_bot_sends_pictures_and_buttons() {
    if server().is_empty() {
        eprintln!("CHATAPP_E2E_SERVER not set, skipping");
        return;
    }
    let _ = env_logger::builder().try_init();
    let base = tempfile::tempdir().unwrap();
    let url = server();
    let owner = device(&base.path().join("owner"));
    let u = url.clone();
    let owner_acc = owner.run(move |i| async move { i.register(&u, PASSWORD, "owner").await }).await.unwrap().account;
    let user = device(&base.path().join("user"));
    let u = url.clone();
    user.run(move |i| async move { i.register(&u, PASSWORD, "user").await }).await.unwrap();

    let name = format!("p{}_bot", &owner_acc.account_id[..8]);
    let created = owner.run(move |i| async move { i.bot_create(&name, "Camera", "").await }).await.unwrap();
    let (bot_id, token) = (created.bot.id.clone(), created.token.clone());
    let id = bot_id.clone();
    let conv = user.run(move |i| async move { i.bot_start(&id).await }).await.unwrap();
    let ups = updates(&token, 0).await;
    let chat = ups[0]["message"]["chat"]["id"].as_str().unwrap().to_string();
    let next = ups[0]["update_id"].as_i64().unwrap() + 1;

    // Buttons: one that answers the bot, one that opens a link. Telegram's spelling works too.
    let (status, sent) = bot_api(
        &token,
        "sendMessage",
        json!({"chat_id": chat, "text": "Motion at the door", "buttons": [[{"text": "Ignore", "data": "ignore:7"}, {"text": "Live", "url": "https://example.org/cam"}]]}),
    )
    .await;
    assert_eq!(status, 200, "{sent}");
    let all = wait_messages(&user, &conv.id, "the message with buttons", |m| m.iter().any(|m| m.text.as_deref() == Some("Motion at the door"))).await;
    let with_buttons = all.iter().find(|m| m.text.as_deref() == Some("Motion at the door")).unwrap().clone();
    assert_eq!(with_buttons.buttons.len(), 1);
    let row = &with_buttons.buttons[0];
    assert_eq!((row[0].text.as_str(), row[0].data.as_deref(), row[0].url.as_deref()), ("Ignore", Some("ignore:7"), None));
    assert_eq!((row[1].text.as_str(), row[1].data.as_deref(), row[1].url.as_deref()), ("Live", None, Some("https://example.org/cam")));
    let (status, _) = bot_api(&token, "sendMessage", json!({"chat_id": chat, "text": "tg", "reply_markup": {"inline_keyboard": [[{"text": "Ok", "callback_data": "ok"}]]}})).await;
    assert_eq!(status, 200);
    // What a button may not be.
    for bad in [json!([[{"text": "x", "url": "http://example.org"}]]), json!([[{"text": "x", "url": "javascript:alert(1)"}]]), json!([[{"text": "x"}]]), json!([[{"text": "", "data": "d"}]])] {
        let (status, body) = bot_api(&token, "sendMessage", json!({"chat_id": chat, "text": "bad", "buttons": bad})).await;
        assert_eq!(status, 400, "{body}");
    }

    // Pressing tells the bot which button of which message; only a real button can be pressed.
    let (mid, wrong) = (with_buttons.id.clone(), with_buttons.id.clone());
    user.run(move |i| async move { i.bot_press(&mid, "ignore:7").await }).await.unwrap();
    assert!(user.run(move |i| async move { i.bot_press(&wrong, "made-up").await }).await.is_err());
    let ups = updates(&token, next).await;
    assert_eq!(ups.len(), 1, "{ups:?}");
    let cb = &ups[0]["callback_query"];
    assert_eq!((cb["data"].as_str(), cb["message_id"].as_str(), cb["chat"]["id"].as_str()), (Some("ignore:7"), Some(with_buttons.id.as_str()), Some(chat.as_str())));

    // A picture, as the request body, with a caption.
    use base64::Engine;
    let png = base64::engine::general_purpose::STANDARD.decode(PNG).unwrap();
    let resp = reqwest::Client::new()
        .post(format!("{}/bot/{token}/sendPhoto?chat_id={chat}&caption=Front%20door", server()))
        .header("Content-Type", "image/png")
        .body(png.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200, "{}", resp.text().await.unwrap_or_default());
    let all = wait_messages(&user, &conv.id, "the picture", |m| {
        m.iter().any(|m| m.text.as_deref() == Some("Front door") && m.attachments.first().is_some_and(|a| a.download_state == "done"))
    })
    .await;
    let pic = all.iter().find(|m| m.text.as_deref() == Some("Front door")).unwrap();
    assert_eq!(pic.kind, "image");
    let a = &pic.attachments[0];
    assert_eq!((a.mime.as_str(), a.width, a.height), ("image/png", Some(1), Some(1)));
    // The local copy is encrypted at rest like all media; the export is the picture itself.
    let out = base.path().join("exported.png");
    let (mid, aid, dest) = (pic.id.clone(), a.id.clone(), out.to_string_lossy().to_string());
    user.run(move |i| async move { i.save_attachment(&mid, &aid, &dest).await }).await.unwrap();
    assert_eq!(std::fs::read(&out).unwrap(), png, "the picture arrives as the bot sent it");

    // As base64 in JSON, without a caption.
    let (status, body) = bot_api(&token, "sendPhoto", json!({"chat_id": chat, "photo": PNG})).await;
    assert_eq!(status, 200, "{body}");
    wait_messages(&user, &conv.id, "the second picture", |m| m.iter().filter(|m| m.kind == "image").count() == 2).await;

    // Not a picture, whatever it calls itself; and nothing reaches a chat that is not the bot's.
    let resp = reqwest::Client::new()
        .post(format!("{}/bot/{token}/sendPhoto?chat_id={chat}", server()))
        .header("Content-Type", "image/png")
        .body("<svg xmlns='http://www.w3.org/2000/svg'/>")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 400);
    let (status, _) = bot_api(&token, "sendPhoto", json!({"chat_id": "0123456789abcdef0123456789abcdef", "photo": PNG})).await;
    assert_eq!(status, 400);

    // After a stop neither pictures nor presses go through.
    let id = bot_id.clone();
    user.run(move |i| async move { i.bot_stop(&id).await }).await.unwrap();
    let (status, body) = bot_api(&token, "sendPhoto", json!({"chat_id": chat, "photo": PNG})).await;
    assert_eq!((status, body["reason"].as_str()), (403, Some("bot_stopped")));
    let mid = with_buttons.id.clone();
    assert_eq!(user.run(move |i| async move { i.bot_press(&mid, "ignore:7").await }).await.unwrap_err().code, ErrorCode::Forbidden);

    owner.shutdown().await;
    user.shutdown().await;
}
