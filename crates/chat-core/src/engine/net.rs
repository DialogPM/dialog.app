//! WebSocket connection, reconnection, presence and server events.

use std::rc::Rc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::{self, Message as WsMessage};

use super::Inner;
use crate::api::Envelope;
use crate::crypto::b64;
use crate::error::{CoreError, ErrorCode, Result};
use crate::model::Presence;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ConnState {
    Connecting,
    Online,
    Offline,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ServerFrame {
    Envelope { envelope: Envelope },
    QueueEmpty,
    Presence { account_id: String, online: bool, last_seen: Option<i64> },
    Event { event: ServerEvent },
    Pong,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ServerEvent {
    DevicesChanged,
    /// A password login waits for approval (or one was answered on another device).
    LoginRequest,
    KeysLow,
    GroupUpdated { group_id: String, revision: i64 },
    AccountDeleted,
    #[serde(other)]
    Unknown,
}

/// How long the socket stays open after the app goes to the background (a quick app switch
/// keeps it), after a push, and after the last message received in the background.
const BG_GRACE: Duration = Duration::from_secs(30);
const BG_PUSH_WINDOW: Duration = Duration::from_secs(20);
const BG_MESSAGE_WINDOW: Duration = Duration::from_secs(10);

enum SocketEnd {
    Closed,
    /// Closed on purpose: the app is in the background (no reconnect until a push or resume).
    Suspended,
    Unauthorized,
    AccountDeleted,
}

impl Inner {
    pub fn set_conn_state(&self, state: ConnState) {
        if self.conn_state.get() != state {
            self.conn_state.set(state);
            self.emit("connection", &state);
        }
    }

    /// A push arrived (mobile): connect now if offline, or make sure the open socket still
    /// works, so the waiting envelopes are fetched promptly.
    pub fn push_wake(&self) {
        self.extend_background(BG_PUSH_WINDOW);
        self.net_wake.notify_one();
    }

    /// Mobile: the app went to the background without a call in progress (true) or came back
    /// (false). In the background the socket (and its ping every 25 s, which keeps the phone's
    /// radio awake) closes after BG_GRACE; pushes then bring the waiting messages.
    pub fn set_background(&self, background: bool) {
        if background {
            if self.bg_deadline.get().is_none() {
                self.bg_deadline.set(Some(tokio::time::Instant::now() + BG_GRACE));
            }
        } else {
            self.bg_deadline.set(None);
            self.net_wake.notify_one();
        }
        self.bg_changed.notify_one();
    }

    /// The user is at this device (`true`: input on the desktop, app in the foreground on a
    /// phone) or away since `since` (ms; now when absent). The server shows the account online
    /// only while one of its devices is active; reconnecting while away stays away.
    pub fn set_presence_active(&self, active: bool, since: Option<i64>) {
        let away = (!active).then(|| since.unwrap_or_else(crate::util::now_ms));
        if self.away_since.get().is_some() == away.is_some() {
            return;
        }
        self.away_since.set(away);
        self.ws_send(json!({ "type": "presence_state", "active": active, "since": away }));
    }

    /// In the background, keeps the socket open at least `window` from now.
    fn extend_background(&self, window: Duration) {
        if let Some(d) = self.bg_deadline.get() {
            self.bg_deadline.set(Some(d.max(tokio::time::Instant::now() + window)));
            self.bg_changed.notify_one();
        }
    }

    /// True while in the background past the deadline: the socket must stay closed.
    fn suspended(&self) -> bool {
        self.bg_deadline.get().is_some_and(|d| tokio::time::Instant::now() >= d)
    }

    /// Sends a JSON frame on the socket if connected. Returns false when offline.
    pub fn ws_send(&self, frame: serde_json::Value) -> bool {
        match self.ws_out.borrow().as_ref() {
            Some(tx) => tx.send(frame.to_string()).is_ok(),
            None => false,
        }
    }

    pub(super) async fn connection_loop(self: Rc<Self>) {
        let mut backoff = Duration::from_secs(1);
        loop {
            if self.suspended() {
                // Background: stay offline until a push (net_wake) or the app comes back.
                *self.ws_out.borrow_mut() = None;
                self.set_conn_state(ConnState::Offline);
                tokio::select! {
                    _ = self.net_wake.notified() => {}
                    _ = self.bg_changed.notified() => {}
                }
                backoff = Duration::from_secs(1);
                continue;
            }
            self.set_conn_state(ConnState::Connecting);
            let started = std::time::Instant::now();
            match self.clone().run_socket().await {
                Ok(SocketEnd::Closed) => {}
                Ok(SocketEnd::Suspended) => continue,
                Ok(SocketEnd::AccountDeleted) => {
                    let _ = self.wipe_local("account_deleted");
                    return;
                }
                Ok(SocketEnd::Unauthorized) => {
                    // The token was revoked: this device was unlinked (or the account deleted).
                    let _ = self.wipe_local("device_removed");
                    return;
                }
                Err(e) => log::warn!("websocket: {e}"),
            }
            *self.ws_out.borrow_mut() = None;
            self.set_conn_state(ConnState::Offline);
            if started.elapsed() > Duration::from_secs(30) {
                backoff = Duration::from_secs(1);
            }
            tokio::select! {
                _ = tokio::time::sleep(backoff) => {
                    backoff = (backoff * 2).min(Duration::from_secs(60));
                }
                // A push arrived: messages wait on the server, try right away.
                _ = self.net_wake.notified() => backoff = Duration::from_secs(1),
                _ = self.bg_changed.notified(), if self.suspended() => {}
            }
        }
    }

    async fn run_socket(self: Rc<Self>) -> Result<SocketEnd> {
        let s = self.session()?;
        let mut url = s.api.ws_url();
        if self.away_since.get().is_some() {
            // Connecting while away: the server does not show the account online meanwhile.
            url.push_str("?active=0");
        }
        let mut req = url
            .into_client_request()
            .map_err(|e| CoreError::network(e.to_string()))?;
        let token = s.api.token().unwrap_or_default().to_string();
        req.headers_mut().insert(
            "Authorization",
            format!("Bearer {token}").parse().map_err(|_| CoreError::internal("bad token"))?,
        );
        let ws = match tokio_tungstenite::connect_async(req).await {
            Ok((ws, _)) => ws,
            Err(tungstenite::Error::Http(resp)) if resp.status() == 401 => return Ok(SocketEnd::Unauthorized),
            Err(e) => return Err(CoreError::network(e.to_string())),
        };
        let (mut sink, mut stream) = ws.split();
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        *self.ws_out.borrow_mut() = Some(tx.clone());

        let writer = tokio::task::spawn_local(async move {
            while let Some(frame) = rx.recv().await {
                if sink.send(WsMessage::Text(frame.into())).await.is_err() {
                    break;
                }
            }
            let _ = sink.close().await;
        });
        let pinger = {
            let tx = tx.clone();
            tokio::task::spawn_local(async move {
                loop {
                    tokio::time::sleep(Duration::from_secs(25)).await;
                    if tx.send(json!({ "type": "ping" }).to_string()).is_err() {
                        break;
                    }
                }
            })
        };
        self.send_presence_subscription();

        let mut end = SocketEnd::Closed;
        // Set while checking that the socket still lives after a push (see push_wake).
        let mut probe: Option<tokio::time::Instant> = None;
        loop {
            // A server that stops talking (no pong for 60 s) is treated as dead.
            let limit = probe.map_or(Duration::from_secs(60), |d| d.saturating_duration_since(tokio::time::Instant::now()));
            let bg_close = self.bg_deadline.get();
            let next = tokio::select! {
                n = tokio::time::timeout(limit, stream.next()) => n,
                // Background deadline reached (it may move: pushes and messages extend it).
                _ = async { tokio::time::sleep_until(bg_close.unwrap()).await }, if bg_close.is_some() => {
                    if self.suspended() {
                        end = SocketEnd::Suspended;
                        break;
                    }
                    continue;
                }
                _ = self.bg_changed.notified() => continue,
                _ = self.net_wake.notified() => {
                    // The server pushed because it could not reach this socket: a phone that
                    // slept may hold a dead connection. Without an answer within 5 s, reconnect.
                    if probe.is_none() && tx.send(json!({ "type": "ping" }).to_string()).is_ok() {
                        probe = Some(tokio::time::Instant::now() + Duration::from_secs(5));
                    }
                    continue;
                }
            };
            probe = None;
            let msg = match next {
                Ok(Some(Ok(m))) => m,
                Ok(Some(Err(e))) => {
                    log::info!("websocket read error: {e}");
                    break;
                }
                Ok(None) | Err(_) => break,
            };
            let text = match msg {
                WsMessage::Text(t) => t.to_string(),
                WsMessage::Close(_) => break,
                _ => continue,
            };
            let frame: ServerFrame = match serde_json::from_str(&text) {
                Ok(f) => f,
                Err(e) => {
                    log::warn!("unknown frame: {e}");
                    continue;
                }
            };
            match frame {
                ServerFrame::Envelope { envelope } => {
                    self.extend_background(BG_MESSAGE_WINDOW);
                    let id = envelope.id.clone();
                    if let Err(e) = self.clone().handle_envelope(envelope).await {
                        log::warn!("envelope {id}: {e}");
                        if e.is_network() {
                            // Keep it queued on the server; it will be redelivered.
                            continue;
                        }
                    }
                    self.ws_send(json!({ "type": "ack", "id": id }));
                }
                ServerFrame::QueueEmpty => {
                    self.set_conn_state(ConnState::Online);
                    self.wake_no_device_jobs(None);
                    self.outbox_notify.notify_one();
                    self.vault_notify.notify_one();
                    // Devices added or logins waiting while we were offline.
                    self.spawn_device_check();
                    self.spawn_login_requests_refresh();
                    let me = self.clone();
                    tokio::task::spawn_local(async move {
                        if let Err(e) = me.refresh_keys().await {
                            log::warn!("key refresh: {e}");
                        }
                        me.sync_all_groups().await;
                    });
                }
                ServerFrame::Presence { account_id, online, last_seen } => {
                    if online {
                        // A peer we could not reach (no device) may be back.
                        self.wake_no_device_jobs(Some(&account_id));
                    }
                    self.emit("presence", &Presence { account_id, online, last_seen });
                }
                ServerFrame::Event { event } => match event {
                    ServerEvent::DevicesChanged => {
                        self.no_other_devices.set(false);
                        self.wake_no_device_jobs(None);
                        self.emit("devices_changed", &serde_json::Value::Null);
                        self.spawn_device_check();
                    }
                    ServerEvent::LoginRequest => self.spawn_login_requests_refresh(),
                    ServerEvent::KeysLow => {
                        let me = self.clone();
                        tokio::task::spawn_local(async move {
                            let _ = me.refresh_keys().await;
                        });
                    }
                    ServerEvent::GroupUpdated { group_id, revision } => {
                        let me = self.clone();
                        tokio::task::spawn_local(async move {
                            if let Err(e) = me.sync_group(&group_id, Some(revision)).await {
                                log::warn!("group sync {group_id}: {e}");
                            }
                        });
                    }
                    ServerEvent::AccountDeleted => {
                        end = SocketEnd::AccountDeleted;
                        break;
                    }
                    ServerEvent::Unknown => {}
                },
                ServerFrame::Pong => {}
            }
        }
        pinger.abort();
        *self.ws_out.borrow_mut() = None;
        writer.abort();
        if matches!(end, SocketEnd::Closed) {
            // Distinguish a revoked token from a network drop.
            if let Ok(s) = self.session() {
                if let Err(e) = s.api.get::<serde_json::Value>("/v1/account").await {
                    if e.code == ErrorCode::Unauthorized {
                        return Ok(SocketEnd::Unauthorized);
                    }
                }
            }
        }
        Ok(end)
    }

    /// (Re)subscribes to presence of accepted direct-chat peers (not message requests) plus
    /// explicitly watched accounts. The server shows an account's presence only to holders of
    /// its access key, derived from the profile key it shares with accepted chats only (see
    /// engine/requests.rs): accounts whose key we do not have are left out.
    pub fn send_presence_subscription(&self) {
        let peers: Vec<String> = self
            .db
            .borrow()
            .conn
            .prepare("SELECT peer_account_id FROM conversations WHERE peer_account_id IS NOT NULL ORDER BY updated_at DESC LIMIT 900")
            .and_then(|mut st| st.query_map([], |r| r.get(0))?.collect())
            .unwrap_or_default();
        let mut ids: Vec<String> = peers.into_iter().filter(|p| self.trusts_account(p)).collect();
        for id in self.presence_watch.borrow().iter() {
            if !ids.contains(id) {
                ids.push(id.clone());
            }
        }
        ids.truncate(1000);
        let subscriptions: Vec<serde_json::Value> = ids
            .iter()
            .filter_map(|id| self.peer_access_key(id).map(|k| json!({ "account_id": id, "access_key": b64(k) })))
            .collect();
        self.ws_send(json!({ "type": "presence_subscribe", "subscriptions": subscriptions }));
    }

    pub fn watch_presence(&self, ids: Vec<String>) {
        {
            let mut w = self.presence_watch.borrow_mut();
            for id in ids {
                w.insert(id);
            }
        }
        self.send_presence_subscription();
    }

    pub(super) async fn maintenance_loop(self: Rc<Self>) {
        loop {
            tokio::time::sleep(Duration::from_secs(6 * 3600)).await;
            if let Err(e) = self.refresh_keys().await {
                log::warn!("key refresh: {e}");
            }
            self.refresh_stale_profiles().await;
        }
    }
}
