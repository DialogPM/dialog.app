//! Development harness: serves one chat core over HTTP so the real web UI can run in a normal
//! browser (Chromium has WebRTC and fake microphones, unlike WebKitGTK on Linux), e.g. to test
//! voice calls end to end between two accounts. Never used in production.
//!
//! `cargo run -p chat-core --example web_bridge -- <port> <data dir> <default server url>`
//!
//! - `POST /invoke/<command>` with the Tauri invoke arguments as JSON: forwarded commands get
//!   `{"req": ...}` (like app/src-tauri/src/commands.rs), `read_media` returns raw bytes,
//!   `ui_log` prints. Errors are 400 with the error text (what Tauri rejects with).
//! - `GET /events`: server-sent events `{"event": ..., "payload": ...}`.
//!
//! The data directory must hold `db.key` (hex), like the desktop app's key-file fallback.
//! app/e2e/tauri-shim.js maps the UI's Tauri IPC onto these endpoints.

use std::path::PathBuf;
use std::sync::Arc;

use chat_core::{Core, CoreConfig, EventSink};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;

struct Events(broadcast::Sender<String>);

impl EventSink for Events {
    fn emit(&self, event: &str, payload: Value) {
        let _ = self.0.send(json!({ "event": event, "payload": payload }).to_string());
    }
}

fn db_key(dir: &std::path::Path) -> [u8; 32] {
    let path = dir.join("db.key");
    let hex = std::fs::read_to_string(&path).unwrap_or_else(|_| {
        let key: String = (0..32).map(|i| format!("{:02x}", (i * 37 + 11) as u8)).collect();
        std::fs::write(&path, &key).expect("write db.key");
        key
    });
    let bytes: Vec<u8> = (0..64)
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex.trim()[i..i + 2], 16).expect("hex db.key"))
        .collect();
    bytes.try_into().expect("32-byte key")
}

const CORS: &str = "Access-Control-Allow-Origin: *\r\nAccess-Control-Allow-Headers: *\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\n";

async fn respond(stream: &mut TcpStream, status: &str, content_type: &str, body: &[u8]) {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n{CORS}Connection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes()).await;
    let _ = stream.write_all(body).await;
}

async fn handle(mut stream: TcpStream, core: Core, data_dir: PathBuf, events: broadcast::Sender<String>) {
    let mut reader = BufReader::new(&mut stream);
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).await.is_err() {
        return;
    }
    let mut length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).await.unwrap_or(0) == 0 || line == "\r\n" {
            break;
        }
        if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            length = v.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0u8; length];
    if reader.read_exact(&mut body).await.is_err() {
        return;
    }
    let mut parts = request_line.split_whitespace();
    let (method, path) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));

    if method == "OPTIONS" {
        return respond(&mut stream, "204 No Content", "text/plain", b"").await;
    }
    if method == "GET" && path == "/events" {
        let head = format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-store\r\n{CORS}\r\n");
        if stream.write_all(head.as_bytes()).await.is_err() {
            return;
        }
        let mut rx = events.subscribe();
        loop {
            match rx.recv().await {
                Ok(ev) => {
                    if stream.write_all(format!("data: {ev}\n\n").as_bytes()).await.is_err() {
                        return;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => eprintln!("events: dropped {n}"),
                Err(_) => return,
            }
        }
    }
    let Some(cmd) = path.strip_prefix("/invoke/") else {
        return respond(&mut stream, "404 Not Found", "text/plain", b"not found").await;
    };
    let args: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    match cmd {
        "read_media" => {
            let requested = PathBuf::from(args["path"].as_str().unwrap_or(""));
            let allowed = requested
                .canonicalize()
                .ok()
                .zip(data_dir.canonicalize().ok())
                .is_some_and(|(p, root)| p.starts_with(root) && p.is_file());
            // Decrypted by the core (media files are encrypted at rest).
            let bytes = if allowed {
                let p = requested.clone();
                core.run(move |i| async move { i.read_media_file(&p).await }).await.ok()
            } else {
                None
            };
            match bytes {
                Some(bytes) => respond(&mut stream, "200 OK", "application/octet-stream", &bytes).await,
                None => respond(&mut stream, "400 Bad Request", "text/plain", b"not_found: media unavailable").await,
            }
        }
        "ui_log" => {
            eprintln!("[ui {}] {}", args["level"].as_str().unwrap_or("info"), args["message"].as_str().unwrap_or(""));
            respond(&mut stream, "200 OK", "application/json", b"null").await
        }
        _ => {
            let req = args.get("req").cloned().unwrap_or(Value::Null);
            match chat_core::rpc::dispatch(&core, cmd, req).await {
                Ok(v) => respond(&mut stream, "200 OK", "application/json", v.to_string().as_bytes()).await,
                Err(e) => respond(&mut stream, "400 Bad Request", "text/plain", e.to_string().as_bytes()).await,
            }
        }
    }
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn,chat_core=info")).try_init();
    let args: Vec<String> = std::env::args().collect();
    let port: u16 = args.get(1).and_then(|p| p.parse().ok()).expect("port");
    let data_dir = PathBuf::from(args.get(2).expect("data dir"));
    let server = args.get(3).cloned().unwrap_or_else(|| "http://127.0.0.1:8080".into());
    std::fs::create_dir_all(&data_dir).expect("data dir");
    let (tx, _) = broadcast::channel(1024);
    let core = Core::start(CoreConfig {
        db_key: db_key(&data_dir),
        data_dir: data_dir.clone(),
        default_server_url: server,
        platform: "desktop".into(),
        events: Arc::new(Events(tx.clone())),
    })
    .expect("start core");
    let listener = TcpListener::bind(("127.0.0.1", port)).await.expect("bind");
    eprintln!("web bridge for {} on http://127.0.0.1:{port}", data_dir.display());
    loop {
        let Ok((stream, _)) = listener.accept().await else { continue };
        tokio::spawn(handle(stream, core.clone(), data_dir.clone(), tx.clone()));
    }
}
