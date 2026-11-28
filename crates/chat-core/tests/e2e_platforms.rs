//! The admin statistics per platform (`GET /v1/admin/platforms`, docs/ADMIN-API.md): a desktop
//! app tells the server which system it runs on when it opens the delivery socket, a phone is
//! told apart by its platform, and the totals count devices and accounts the way the admin
//! panel shows them. The admin requests are signed with the test admin key that
//! scripts/test-e2e.sh gives the server; device rows are prepared through its MySQL database.
//!
//! `scripts/test-e2e.sh --test e2e_platforms`

use std::sync::{Arc, Mutex};
use std::time::Duration;

use chat_core::crypto::b64;
use chat_core::{Core, CoreConfig, EventSink};
use ed25519_dalek::{Signer, SigningKey};
use serde_json::Value;
use sha2::{Digest, Sha256};

const DB_KEY: [u8; 32] = [7u8; 32];
const PASSWORD: &str = "correct horse battery";
const DAY_MS: i64 = 86_400_000;

#[derive(Default)]
struct Events(Mutex<Vec<(String, Value)>>);

impl EventSink for Events {
    fn emit(&self, event: &str, payload: Value) {
        self.0.lock().unwrap().push((event.to_string(), payload));
    }
}

struct Client {
    core: Core,
    events: Arc<Events>,
    _dir: tempfile::TempDir,
}

impl Client {
    fn new(platform: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let events = Arc::new(Events::default());
        let core = Core::start(CoreConfig {
            data_dir: dir.path().to_path_buf(),
            db_key: DB_KEY,
            default_server_url: server(),
            platform: platform.into(),
            events: events.clone(),
        })
        .unwrap();
        Self { core, events, _dir: dir }
    }

    /// Registers an account and waits for the delivery socket; returns the account id.
    async fn register(&self) -> String {
        let u = server();
        let r = self.core.run(move |i| async move { i.register(&u, PASSWORD, "test device").await }).await.unwrap();
        for _ in 0..300 {
            if self.events.0.lock().unwrap().iter().any(|(n, v)| n == "connection" && v.as_str() == Some("online")) {
                return r.account.account_id;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("timed out waiting for the connection");
    }
}

fn server() -> String {
    std::env::var("CHATAPP_E2E_SERVER").unwrap_or_default()
}

fn skip() -> bool {
    if server().is_empty() || std::env::var("CHATAPP_E2E_MYSQL").is_err() {
        eprintln!("CHATAPP_E2E_SERVER / CHATAPP_E2E_MYSQL not set, skipping");
        return true;
    }
    let _ = env_logger::builder().is_test(true).try_init();
    false
}

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

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64
}

/// A signed request to the admin API (docs/ADMIN-API.md); returns the status and the body.
async fn admin_get(path: &str) -> (u16, Value) {
    let ts = now_ms().to_string();
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let message = format!("dialog-admin-v1\nGET\n{path}\n{ts}\n{nonce}\n{}", hex::encode(Sha256::digest(b"")));
    let key = SigningKey::from_bytes(&std::array::from_fn(|i| i as u8));
    let r = reqwest::Client::new()
        .get(format!("{}{path}", server()))
        .header("X-Admin-Key", b64(key.verifying_key().to_bytes()))
        .header("X-Admin-Timestamp", &ts)
        .header("X-Admin-Nonce", &nonce)
        .header("X-Admin-Signature", b64(key.sign(message.as_bytes()).to_bytes()))
        .send()
        .await
        .unwrap();
    let status = r.status().as_u16();
    (status, serde_json::from_str(&r.text().await.unwrap()).unwrap_or(Value::Null))
}

/// The four totals of one platform over `days` days.
#[derive(Clone, Copy, PartialEq, Debug)]
struct Totals {
    devices: i64,
    new_devices: i64,
    active_devices: i64,
    active_accounts: i64,
}

struct Platforms {
    active_accounts: i64,
    items: Vec<(String, Totals)>,
}

impl Platforms {
    async fn get(days: i64) -> Self {
        let (status, v) = admin_get(&format!("/v1/admin/platforms?days={days}")).await;
        assert_eq!(status, 200, "{v}");
        assert_eq!(v["days"], days);
        let n = |i: &Value, k: &str| i[k].as_i64().unwrap_or_else(|| panic!("{k} missing in {i}"));
        let items = v["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| {
                let t = Totals { devices: n(i, "devices"), new_devices: n(i, "new_devices"), active_devices: n(i, "active_devices"), active_accounts: n(i, "active_accounts") };
                (i["platform"].as_str().unwrap().to_string(), t)
            })
            .collect();
        Self { active_accounts: v["active_accounts"].as_i64().unwrap(), items }
    }

    fn of(&self, platform: &str) -> Totals {
        self.items.iter().find(|(p, _)| p == platform).unwrap_or_else(|| panic!("{platform} missing")).1
    }

    /// What changed for `platform` since `before`, as (devices, new, active devices, active accounts).
    fn delta(&self, before: &Platforms, platform: &str) -> (i64, i64, i64, i64) {
        let (a, b) = (self.of(platform), before.of(platform));
        (a.devices - b.devices, a.new_devices - b.new_devices, a.active_devices - b.active_devices, a.active_accounts - b.active_accounts)
    }
}

fn device_os(account: &str) -> String {
    sql(&format!("SELECT COALESCE(os, 'NULL') FROM devices WHERE account_id = '{account}' AND device_id = 1"))
}

/// A second device row of an account, as another linked device would leave it.
fn add_device(account: &str, device_id: i64, platform: &str, os: Option<&str>, created_at: i64, last_seen_at: i64) {
    let os = os.map(|o| format!("'{o}'")).unwrap_or_else(|| "NULL".into());
    sql(&format!(
        "INSERT INTO devices (account_id, device_id, name, platform, os, registration_id, token_hash, created_at, last_seen_at)
         VALUES ('{account}', {device_id}, 'Dialog', '{platform}', {os}, 77, UNHEX('{}'), {created_at}, {last_seen_at})",
        hex::encode(Sha256::digest(uuid::Uuid::new_v4().as_bytes()))
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn devices_and_accounts_are_counted_per_platform() {
    if skip() {
        return;
    }
    let this_os = std::env::consts::OS;
    assert!(["windows", "macos", "linux"].contains(&this_os), "the test runs on a desktop system");

    let before = Platforms::get(7).await;
    let names: Vec<&str> = before.items.iter().map(|(p, _)| p.as_str()).collect();
    assert_eq!(names, ["android", "ios", "windows", "macos", "linux", "desktop", "other"], "every group, in a fixed order");

    // A desktop app says which system it runs on; a phone says nothing more than its platform.
    let desktop = Client::new("desktop");
    let d = desktop.register().await;
    let phone = Client::new("android");
    let p = phone.register().await;
    assert_eq!(device_os(&d), this_os);
    assert_eq!(device_os(&p), "NULL", "only desktops have a system stored");

    let after = Platforms::get(7).await;
    assert_eq!(after.delta(&before, this_os), (1, 1, 1, 1));
    assert_eq!(after.delta(&before, "android"), (1, 1, 1, 1));
    assert_eq!(after.delta(&before, "ios"), (0, 0, 0, 0));
    assert_eq!(after.delta(&before, "desktop"), (0, 0, 0, 0));
    assert_eq!(after.active_accounts - before.active_accounts, 2);

    // A desktop that has not said its system (an older app) is shown as such, not hidden and
    // not guessed; a value outside the list in the database counts the same way.
    sql(&format!("UPDATE devices SET os = NULL WHERE account_id = '{d}'"));
    let unknown = Platforms::get(7).await;
    assert_eq!(unknown.delta(&before, this_os), (0, 0, 0, 0));
    assert_eq!(unknown.delta(&before, "desktop"), (1, 1, 1, 1));
    sql(&format!("UPDATE devices SET os = 'beos' WHERE account_id = '{d}'"));
    assert_eq!(Platforms::get(7).await.delta(&before, "desktop"), (1, 1, 1, 1));
    sql(&format!("UPDATE devices SET os = '{this_os}' WHERE account_id = '{d}'"));

    // An account with several devices counts once per platform it was active on: the phone's
    // account gets a second Android phone and an iPhone, the desktop's account a Linux and a
    // Windows computer linked long ago, one of them not seen in the period.
    let now = now_ms();
    let (old, recent) = (now - 40 * DAY_MS, now - DAY_MS);
    add_device(&p, 2, "android", None, recent, recent);
    add_device(&p, 3, "ios", None, recent, recent);
    let other_a = if this_os == "linux" { "macos" } else { "linux" };
    let other_b = if this_os == "windows" { "macos" } else { "windows" };
    add_device(&d, 2, "desktop", Some(other_a), old, recent);
    add_device(&d, 3, "desktop", Some(other_b), old, old);
    add_device(&d, 4, "cli", None, recent, recent);

    let week = Platforms::get(7).await;
    assert_eq!(week.delta(&before, "android"), (2, 2, 2, 1), "two phones, one account");
    assert_eq!(week.delta(&before, "ios"), (1, 1, 1, 1), "the same account counts for iOS too");
    assert_eq!(week.delta(&before, this_os), (1, 1, 1, 1));
    assert_eq!(week.delta(&before, other_a), (1, 0, 1, 1), "linked before the period, seen in it");
    assert_eq!(week.delta(&before, other_b), (1, 0, 0, 0), "linked and last seen before the period");
    assert_eq!(week.delta(&before, "other"), (1, 1, 1, 1), "web and cli devices are 'other'");
    assert_eq!(week.active_accounts - before.active_accounts, 2, "the total counts each account once");

    // The period: yesterday's devices are outside `days=1` (today, UTC), the totals stay.
    let today_before = Platforms::get(1).await;
    assert_eq!(today_before.of("ios").devices, week.of("ios").devices);
    sql(&format!("UPDATE devices SET created_at = {old}, last_seen_at = {old} WHERE account_id = '{p}' AND device_id = 3"));
    let long = Platforms::get(90).await;
    let short = Platforms::get(7).await;
    assert_eq!(short.delta(&week, "ios"), (0, -1, -1, -1));
    assert_eq!(long.of("ios").new_devices - short.of("ios").new_devices, 1, "40 days ago is inside 90 days");

    // The same bounds as the other statistics.
    assert_eq!(admin_get("/v1/admin/platforms?days=0").await.0, 400);
    assert_eq!(admin_get("/v1/admin/platforms?days=366").await.0, 400);
    assert_eq!(admin_get("/v1/admin/platforms").await.1["days"], 30);
}
