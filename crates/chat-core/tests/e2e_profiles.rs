//! The phone scenario of the multi-account profiles, against a real server: an account whose
//! only device is this phone must stay reachable while the user switches to another account on
//! the same phone, and get its messages when it is opened again.
//!
//! `CHATAPP_E2E_SERVER=http://127.0.0.1:8081 cargo test -p chat-core --test e2e_profiles`

use std::sync::{Arc, Mutex};
use std::time::Duration;

use chat_core::model::*;
use chat_core::profiles::Profiles;
use chat_core::EventSink;
use serde_json::{json, Value};

#[derive(Default)]
struct Events(Mutex<Vec<(String, Value)>>);

impl EventSink for Events {
    fn emit(&self, event: &str, payload: Value) {
        self.0.lock().unwrap().push((event.to_string(), payload));
    }
}

impl Events {
    async fn wait(&self, what: &str, pred: impl Fn(&str, &Value) -> bool) -> Value {
        for _ in 0..300 {
            if let Some((_, v)) = self.0.lock().unwrap().iter().find(|(n, v)| pred(n, v)) {
                return v.clone();
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("timed out waiting for {what}");
    }

    fn clear(&self) {
        self.0.lock().unwrap().clear();
    }
}

fn server() -> String {
    std::env::var("CHATAPP_E2E_SERVER").unwrap_or_default()
}

const PW_A: &str = "Alin-phone-only-device-1";
const PW_B: &str = "Mass-second-account-on-phone-2";

/// Creates an account on the active (blank) profile and protects its local data, as the
/// onboarding does. Returns the account.
async fn create_account(phone: &Profiles, events: &Events, password: &'static str) -> AccountInfo {
    let url = server();
    let r = phone.core().run(move |i| async move { i.register(&url, password, "iPhone").await }).await.unwrap();
    // Through the same entry point as the apps, so the profile list is updated.
    phone.invoke("confirm_mnemonic_saved", Value::Null).await.unwrap();
    phone.invoke("protect_local_data", json!({ "password": password })).await.unwrap();
    events.wait("online", |n, v| n == "connection" && v.as_str() == Some("online")).await;
    r.account
}

async fn device_count(phone: &Profiles) -> usize {
    phone.core().run(|i| async move { i.list_devices().await }).await.unwrap().len()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn switching_account_keeps_the_other_one_reachable() {
    if server().is_empty() {
        eprintln!("CHATAPP_E2E_SERVER not set, skipping");
        return;
    }
    let _ = env_logger::builder().try_init();
    let dir = tempfile::tempdir().unwrap();
    let events = Arc::new(Events::default());
    let phone = Profiles::start(dir.path().to_path_buf(), server(), "ios".into(), events.clone()).unwrap();

    // 1. "alin" is created on the phone: the phone is its only device.
    let alin = create_account(&phone, &events, PW_A).await;
    assert_eq!(device_count(&phone).await, 1);
    let alin_profile = phone.list().into_iter().find(|p| p.last4.as_deref() == Some(&alin.number[12..])).map(|p| p.id);
    let alin_profile = alin_profile.expect("alin is on the start screen");

    // 2. "Switch account" (not log out), then a second account on the same phone.
    phone.invoke("profile_close", Value::Null).await.unwrap();
    events.clear();
    let mass = create_account(&phone, &events, PW_B).await;
    assert_ne!(alin.account_id, mass.account_id);
    assert_eq!(phone.list().len(), 2, "both accounts are listed on the start screen");

    // alin still has its device on the server: messages to it can be encrypted and queued.
    let alin_number = alin.number.clone();
    let contact = phone.core().run(move |i| async move { i.add_contact_by_number(&alin_number, Some("alin".into())).await }).await.unwrap();
    let alin_id = contact.account_id.clone();
    let conv = phone.core().run(move |i| async move { i.open_direct_conversation(&alin_id).await }).await.unwrap();
    let req: SendMessageRequest = serde_json::from_value(json!({ "conversation_id": conv.id, "text": "salut alin" })).unwrap();
    let sent = phone.core().run(move |i| async move { i.send_message(req).await }).await.unwrap();
    let sent_id = sent.id.clone();
    let status = events
        .wait("sent to alin", move |n, v| {
            n == "message" && v["message"]["id"].as_str() == Some(sent_id.as_str()) && v["message"]["status"].as_str() == Some("sent")
        })
        .await;
    assert!(status["message"]["status_reason"].is_null(), "not waiting for a device: {status}");

    // 3. Back to alin with its password: the message waiting on the server arrives.
    let e = phone.invoke("profile_open", json!({ "id": alin_profile, "password": "wrong password!!" })).await.unwrap_err();
    assert!(e.starts_with("invalid_password"), "{e}");
    events.clear();
    phone.invoke("profile_open", json!({ "id": alin_profile, "password": PW_A })).await.unwrap();
    // (Status updates of Mass's own copy may still arrive while it shuts down: wait for the
    // incoming one.)
    let got = events
        .wait("message for alin", |n, v| {
            n == "message" && v["message"]["text"].as_str() == Some("salut alin") && v["message"]["outgoing"] == json!(false)
        })
        .await;
    assert_eq!(got["message"]["outgoing"], json!(false));
    assert_eq!(device_count(&phone).await, 1, "alin kept its only device");

    // 4. And the other way round: alin answers while Mass is closed, Mass gets it when opened.
    let mass_conv = got["message"]["conversation_id"].as_str().unwrap().to_string();
    let c = mass_conv.clone();
    phone.core().run(move |i| async move { i.accept_conversation(&c).await }).await.unwrap();
    let req: SendMessageRequest = serde_json::from_value(json!({ "conversation_id": mass_conv, "text": "salut Mass" })).unwrap();
    let reply = phone.core().run(move |i| async move { i.send_message(req).await }).await.unwrap();
    let reply_id = reply.id.clone();
    events
        .wait("reply sent", move |n, v| {
            n == "message" && v["message"]["id"].as_str() == Some(reply_id.as_str()) && v["message"]["status"].as_str() == Some("sent")
        })
        .await;
    let mass_profile = phone.list().into_iter().find(|p| p.last4.as_deref() == Some(&mass.number[12..])).unwrap().id;
    events.clear();
    phone.invoke("profile_open", json!({ "id": mass_profile, "password": PW_B })).await.unwrap();
    events
        .wait("reply for Mass", |n, v| {
            n == "message" && v["message"]["text"].as_str() == Some("salut Mass") && v["message"]["outgoing"] == json!(false)
        })
        .await;

    phone.exit().await;
}

/// Quick switch: an account opened with its password once is switched to again without it,
/// until the app quits; after a restart the password is asked again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn quick_switch_without_password_until_restart() {
    if server().is_empty() {
        eprintln!("CHATAPP_E2E_SERVER not set, skipping");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let events = Arc::new(Events::default());
    let phone = Profiles::start(dir.path().to_path_buf(), server(), "ios".into(), events.clone()).unwrap();
    let a = create_account(&phone, &events, PW_A).await;
    phone.invoke("profile_close", Value::Null).await.unwrap();
    events.clear();
    let b = create_account(&phone, &events, PW_B).await;
    let id_of = |p: &Profiles, n: &str| p.list().into_iter().find(|c| c.last4.as_deref() == Some(&n[12..])).unwrap();
    let account = |p: &Profiles| p.core();

    // Both were set up in this run: both switch without a password.
    let card_a = id_of(&phone, &a.number);
    assert!(card_a.unlocked && !card_a.active);
    assert!(id_of(&phone, &b.number).active);
    phone.invoke("profile_switch", json!({ "id": card_a.id })).await.unwrap();
    let now = account(&phone).run(|i| async move { Ok(i.app_state()) }).await.unwrap();
    assert_eq!(now.account.unwrap().account_id, a.account_id);
    let card_b = id_of(&phone, &b.number);
    phone.invoke("profile_switch", json!({ "id": card_b.id })).await.unwrap();
    let now = account(&phone).run(|i| async move { Ok(i.app_state()) }).await.unwrap();
    assert_eq!(now.account.unwrap().account_id, b.account_id);
    // Through the start screen too ("Switch account" keeps the keys).
    phone.invoke("profile_close", Value::Null).await.unwrap();
    phone.invoke("profile_switch", json!({ "id": card_a.id })).await.unwrap();
    phone.exit().await;

    // After a restart: the password is required again, then quick switching works again.
    let phone = Profiles::start(dir.path().to_path_buf(), server(), "ios".into(), events.clone()).unwrap();
    assert!(phone.list().iter().all(|c| !c.unlocked));
    let e = phone.invoke("profile_switch", json!({ "id": card_a.id })).await.unwrap_err();
    assert!(e.starts_with("password_required"), "{e}");
    phone.invoke("profile_open", json!({ "id": card_a.id, "password": PW_A })).await.unwrap();
    phone.invoke("profile_open", json!({ "id": card_b.id, "password": PW_B })).await.unwrap();
    phone.invoke("profile_switch", json!({ "id": card_a.id })).await.unwrap();
    let now = phone.core().run(|i| async move { Ok(i.app_state()) }).await.unwrap();
    assert_eq!(now.account.unwrap().account_id, a.account_id);
    phone.exit().await;
}
