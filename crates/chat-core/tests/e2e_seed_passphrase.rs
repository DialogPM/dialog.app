//! Seed passphrase (the optional "25th word") against a real server: an account created
//! with one is recovered only with the 24 words *and* the passphrase, its words can still be
//! shown on every device, and accounts without a passphrase behave as before.
//!
//! `CHATAPP_E2E_SERVER=http://127.0.0.1:8081 cargo test -p chat-core --test e2e_seed_passphrase`

use std::path::Path;
use std::sync::Arc;

use chat_core::{Core, CoreConfig, ErrorCode, EventSink};
use serde_json::Value;

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

const PASSWORD: &str = "Seed-passphrase-test-1";
const PASSPHRASE: &str = "al 25-lea cuvânt";

async fn words_shown(core: &Core, password: &'static str) -> Vec<String> {
    core.run(move |i| async move { i.reveal_mnemonic(password).await }).await.unwrap()
}

async fn has_passphrase(core: &Core) -> bool {
    core.run(|i| async move { Ok(i.app_state().seed_passphrase) }).await.unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn account_with_a_seed_passphrase() {
    if server().is_empty() {
        eprintln!("CHATAPP_E2E_SERVER not set, skipping");
        return;
    }
    let _ = env_logger::builder().try_init();
    let base = tempfile::tempdir().unwrap();
    let url = server();

    // Created with a passphrase: the result says so and the 24 words are the usual ones.
    let first = device(&base.path().join("first"));
    let u = url.clone();
    let r = first
        .run(move |i| async move { i.register_with_passphrase(&u, PASSWORD, "first", PASSPHRASE).await })
        .await
        .unwrap();
    assert!(r.seed_passphrase);
    assert_eq!(r.mnemonic.len(), 24);
    assert!(has_passphrase(&first).await);
    assert_eq!(words_shown(&first, PASSWORD).await, r.mnemonic, "the words shown again are the ones written down");

    // A second device signing in with the password gets the same account and the same words.
    let second = device(&base.path().join("second"));
    let (u, number) = (url.clone(), r.account.number.clone());
    let acc = second.run(move |i| async move { i.login(&u, &number, PASSWORD, "second", None).await }).await.unwrap();
    assert_eq!(acc.account_id, r.account.account_id);
    assert!(has_passphrase(&second).await);
    assert_eq!(words_shown(&second, PASSWORD).await, r.mnemonic);

    // Changing the password keeps the words.
    const CHANGED: &str = "Seed-passphrase-changed-2";
    first.run(|i| async move { i.change_password(PASSWORD, CHANGED).await }).await.unwrap();
    assert_eq!(words_shown(&first, CHANGED).await, r.mnemonic);

    // A device linked by QR code receives the words from the device that approves it.
    let linked = Arc::new(device(&base.path().join("linked")));
    let u = url.clone();
    let link = linked.run(move |i| async move { i.link_create(&u, "linked").await }).await.unwrap();
    let waiter = {
        let core = linked.clone();
        tokio::spawn(async move { core.run(|i| async move { i.link_wait().await }).await })
    };
    first.run(move |i| async move { i.link_approve(Some(&link)).await }).await.unwrap();
    assert_eq!(waiter.await.unwrap().unwrap().account_id, r.account.account_id);
    assert!(has_passphrase(&linked).await);
    assert_eq!(words_shown(&linked, CHANGED).await, r.mnemonic);

    // Recovery: the 24 words alone, or with another passphrase, are some other account's keys.
    for (n, wrong) in ["", "al 25-lea cuvant", "Al 25-lea cuvânt"].into_iter().enumerate() {
        let dev = device(&base.path().join(format!("wrong{n}")));
        let (u, number, words) = (url.clone(), r.account.number.clone(), r.mnemonic.clone());
        let err = dev
            .run(move |i| async move {
                i.recover_with_passphrase(&u, &number, &words, wrong, "Recovered-password-3", "thief").await
            })
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidMnemonic, "passphrase {wrong:?}");
        dev.shutdown().await;
    }

    // With the passphrase the account comes back, words included.
    const RECOVERED: &str = "Recovered-password-3";
    let third = device(&base.path().join("third"));
    let (u, number, words) = (url.clone(), r.account.number.clone(), r.mnemonic.clone());
    let acc = third
        .run(move |i| async move { i.recover_with_passphrase(&u, &number, &words, PASSPHRASE, RECOVERED, "third").await })
        .await
        .unwrap();
    assert_eq!(acc.account_id, r.account.account_id);
    assert!(has_passphrase(&third).await);
    assert_eq!(words_shown(&third, RECOVERED).await, r.mnemonic);

    first.shutdown().await;
    second.shutdown().await;
    linked.shutdown().await;
    third.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn account_without_a_seed_passphrase() {
    if server().is_empty() {
        eprintln!("CHATAPP_E2E_SERVER not set, skipping");
        return;
    }
    let _ = env_logger::builder().try_init();
    let base = tempfile::tempdir().unwrap();
    let url = server();

    let first = device(&base.path().join("first"));
    let u = url.clone();
    let r = first.run(move |i| async move { i.register(&u, PASSWORD, "first").await }).await.unwrap();
    assert!(!r.seed_passphrase);
    assert!(!has_passphrase(&first).await);
    assert_eq!(words_shown(&first, PASSWORD).await, r.mnemonic);

    // A passphrase that was never set does not open the account.
    let thief = device(&base.path().join("thief"));
    let (u, number, words) = (url.clone(), r.account.number.clone(), r.mnemonic.clone());
    let err = thief
        .run(move |i| async move { i.recover_with_passphrase(&u, &number, &words, "anything", "Recovered-password-3", "x").await })
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidMnemonic);

    // The words alone do, as always.
    const RECOVERED: &str = "Recovered-password-3";
    let again = device(&base.path().join("again"));
    let (u, number, words) = (url.clone(), r.account.number.clone(), r.mnemonic.clone());
    let acc = again.run(move |i| async move { i.recover(&u, &number, &words, RECOVERED, "again").await }).await.unwrap();
    assert_eq!(acc.account_id, r.account.account_id);
    assert!(!has_passphrase(&again).await);
    assert_eq!(words_shown(&again, RECOVERED).await, r.mnemonic);

    first.shutdown().await;
    thief.shutdown().await;
    again.shutdown().await;
}
