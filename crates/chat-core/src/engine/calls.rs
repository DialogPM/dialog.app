//! Voice and video calls: signaling transport and call history.
//!
//! The media path (WebRTC, DTLS-SRTP) lives in the UI; the core only carries the signaling
//! messages end-to-end encrypted over the Signal sessions, filters what the UI should see
//! (blocked callers, signals for another device, stale offers) and records calls in the
//! conversation.
//!
//! Calls from accounts whose conversation is not accepted (message requests, see
//! engine/requests.rs) never ring: the offer becomes a missed call and the caller is told
//! `hangup` with reason `need_permission`. Offers from such strangers are also limited per
//! caller, so nobody can flood the call history; accepted conversations are never limited.
//!
//! Do not disturb (engine/dnd.rs): offers do not ring and become missed calls, and nothing goes
//! back to the caller, whose ring timeout ends the call.
//!
//! Calls turned off (engine/call_settings.rs): offers do not ring and leave no history; the
//! caller's device is told `hangup` with reason `calls_disabled`. Relay-only calls: every call
//! event says `relay`, and what we send carries relay candidates only.

use std::rc::Rc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::send::Job;
use super::{call_checks, Inner};
use crate::content::{CallSignal, Content};
use crate::error::{CoreError, ErrorCode, Result};
use crate::util::now_ms;

/// An offer older than this is not rung anymore; it becomes a missed call.
const OFFER_MAX_AGE_MS: i64 = 60_000;
/// Other signals older than this belong to a call that is over.
const SIGNAL_MAX_AGE_MS: i64 = 120_000;
/// "media": camera turned on / off during a video call (no renegotiation).
/// "reoffer": ICE restart of a call in progress (the caller's new offer after the connection
/// dropped, e.g. a network change); answered with a plain "answer". Unlike "offer" it never
/// rings, never counts as a new call and is never turned into a missed call: the UI applies it
/// only to the call it is in, and only from the device it talks to.
const KINDS: &[&str] = &["offer", "answer", "ice", "ringing", "hangup", "busy", "media", "reoffer"];
/// "unavailable": the callee does not accept calls (hangup reason `calls_disabled`).
const STATUSES: &[&str] = &["answered", "missed", "declined", "busy", "failed", "cancelled", "unavailable"];
/// New calls (distinct call ids) accepted per stranger (conversation not accepted) in
/// `OFFER_WINDOW_MS`; more are dropped without a trace in the history.
const OFFERS_PER_WINDOW: usize = 3;
const OFFER_WINDOW_MS: i64 = 60_000;

#[derive(Debug, Deserialize)]
pub struct CallSignalRequest {
    /// Peer account (ignored when `to_own_devices`).
    pub account_id: String,
    pub call_id: String,
    pub kind: String,
    #[serde(default)]
    pub data: Value,
    pub target_device: Option<u32>,
    /// Send to our own other devices instead (e.g. "answered elsewhere").
    #[serde(default)]
    pub to_own_devices: bool,
}

#[derive(Debug, Deserialize)]
pub struct RecordCallRequest {
    pub conversation_id: String,
    pub call_id: String,
    /// Account that placed the call.
    pub caller_account_id: String,
    pub status: String,
    #[serde(default)]
    pub duration_ms: i64,
    pub started_at: i64,
}

/// Payload of the `call_signal` event.
#[derive(Debug, Serialize)]
pub struct CallSignalEvent {
    pub account_id: String,
    pub device_id: u32,
    pub conversation_id: Option<String>,
    pub call_id: String,
    pub kind: String,
    pub data: Value,
    /// Sent by one of our own devices (e.g. answered or declined there).
    pub from_me: bool,
    /// The caller is not in our contacts, or we take calls through the relay only
    /// (engine/call_settings.rs): media must go through the TURN relay only
    /// (iceTransportPolicy "relay"), so the peer does not learn our IP address.
    pub relay: bool,
}

fn valid_call_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

impl Inner {
    pub async fn call_signal(self: &Rc<Self>, req: CallSignalRequest) -> Result<()> {
        if !valid_call_id(&req.call_id) || !KINDS.contains(&req.kind.as_str()) {
            return Err(CoreError::invalid("invalid call signal"));
        }
        let me = self.me()?;
        let to = if req.to_own_devices { me.clone() } else { req.account_id.clone() };
        if !req.to_own_devices && (to == me || self.is_blocked(&to)) {
            return Err(CoreError::invalid("cannot call this account"));
        }
        if req.kind == "offer" && !req.to_own_devices {
            if !self.calls_allowed() {
                return Err(CoreError::new(ErrorCode::Forbidden, "calls are turned off in settings"));
            }
            // Calling someone accepts their message request.
            if let Some(conv) = self.conversation_id_for_peer(&to)? {
                self.accept_on_send(&conv)?;
            }
        }
        let mut data = req.data;
        if !req.to_own_devices && self.relay_only() {
            match req.kind.as_str() {
                "offer" | "answer" | "reoffer" => data = call_checks::relay_only_sdp_signal(&data),
                "ice" => match call_checks::relay_only_ice_signal(&data) {
                    Some(d) => data = d,
                    None => return Ok(()),
                },
                _ => {}
            }
        }
        // A restart is for the device of the call in progress, never a broadcast.
        if req.kind == "reoffer" && (req.to_own_devices || req.target_device.is_none()) {
            return Err(CoreError::invalid("a call restart needs its target device"));
        }
        let signal = CallSignal { call_id: req.call_id, kind: req.kind, data, target_device: req.target_device, group_id: None };
        // Stored by the server (not "online only"): a callee that is briefly offline still
        // gets the offer, and late offers turn into missed calls on arrival.
        self.send_to_account(&to, &Content::Call(signal), false).await
    }

    pub(super) async fn handle_call_signal(&self, from: &str, device: u32, sent_at: i64, mut s: CallSignal) -> Result<()> {
        let session = self.session()?;
        if s.target_device.is_some_and(|d| d != session.account.device_id) {
            return Ok(());
        }
        if !valid_call_id(&s.call_id) || !KINDS.contains(&s.kind.as_str()) {
            return Ok(());
        }
        // Remote SDP and ICE go straight into WebRTC: only what our clients produce passes.
        match s.kind.as_str() {
            "offer" | "answer" | "reoffer" if !call_checks::valid_sdp_signal(&s.data) => return Ok(()),
            // A restart names the device of the call in progress.
            "reoffer" if s.target_device.is_none() => return Ok(()),
            "ice" => match call_checks::clean_ice_signal(&s.data) {
                Some(data) => s.data = data,
                None => return Ok(()),
            },
            _ => {}
        }
        let from_me = from == session.account.account_id;
        if from_me && s.kind != "hangup" && s.kind != "answer" {
            return Ok(());
        }
        if !from_me && self.is_blocked(from) {
            return Ok(());
        }
        if !from_me && s.kind == "offer" && !self.trusts_account(from) && !self.allow_call_offer(from, &s.call_id) {
            log::warn!("dropping call offer from {from}: too many calls");
            return Ok(());
        }
        // `sent_at` is the server's receive time (never the sender's clock); none = too old.
        let Some(age) = call_checks::signal_age(now_ms(), sent_at) else { return Ok(()) };
        // Calls turned off: no ring, no history (not even a conversation with a stranger); a
        // caller still waiting learns it at once.
        if !from_me && s.kind == "offer" && !self.calls_allowed() {
            if age <= OFFER_MAX_AGE_MS {
                let reply = CallSignal {
                    call_id: s.call_id,
                    kind: "hangup".into(),
                    data: json!({ "reason": "calls_disabled" }),
                    target_device: Some(device),
                    group_id: None,
                };
                self.enqueue(&Job::Account { account_id: from.to_string(), content: Content::Call(reply) })?;
            }
            return Ok(());
        }
        let conversation_id = if from_me { None } else { Some(self.ensure_direct_conversation(from, false).await?) };
        if s.kind == "offer" && age > OFFER_MAX_AGE_MS {
            if let Some(conv) = &conversation_id {
                self.record_call_message(conv, &s.call_id, from, "missed", 0, sent_at)?;
            }
            return Ok(());
        }
        if age > SIGNAL_MAX_AGE_MS {
            return Ok(());
        }
        // Message request: the call does not ring. The caller learns it needs permission (no
        // `ringing`/`busy` that would tell it more); other signals from it mean nothing to us.
        if let Some(conv) = conversation_id.as_deref().filter(|c| !self.conversation_accepted_id(c)) {
            if s.kind == "offer" {
                self.record_call_message(conv, &s.call_id, from, "missed", 0, sent_at)?;
                let reply = CallSignal {
                    call_id: s.call_id,
                    kind: "hangup".into(),
                    data: json!({ "reason": "need_permission" }),
                    target_device: Some(device),
                    group_id: None,
                };
                self.enqueue(&Job::Account { account_id: from.to_string(), content: Content::Call(reply) })?;
            }
            return Ok(());
        }
        if s.kind == "offer" && !from_me && self.dnd_active() {
            if let Some(conv) = &conversation_id {
                self.record_call_message(conv, &s.call_id, from, "missed", 0, sent_at)?;
            }
            return Ok(());
        }
        let relay = !from_me && (self.relay_only() || !self.is_contact(from));
        self.emit(
            "call_signal",
            &CallSignalEvent {
                account_id: from.to_string(),
                device_id: device,
                conversation_id,
                call_id: s.call_id,
                kind: s.kind,
                data: s.data,
                from_me,
                relay,
            },
        );
        Ok(())
    }

    /// Counts a new call from `from` (a stranger); false when it exceeds the per-caller limit.
    /// A repeated offer for a call already counted is let through.
    pub(super) fn allow_call_offer(&self, from: &str, call_id: &str) -> bool {
        let now = now_ms();
        let mut offers = self.call_offers.borrow_mut();
        offers.retain(|_, calls| {
            calls.retain(|(_, t)| now - t < OFFER_WINDOW_MS);
            !calls.is_empty()
        });
        let calls = offers.entry(from.to_string()).or_default();
        if calls.iter().any(|(id, _)| id == call_id) {
            return true;
        }
        if calls.len() >= OFFERS_PER_WINDOW {
            return false;
        }
        calls.push((call_id.to_string(), now));
        true
    }

    pub fn record_call(&self, req: RecordCallRequest) -> Result<()> {
        if !valid_call_id(&req.call_id) || !STATUSES.contains(&req.status.as_str()) {
            return Err(CoreError::invalid("invalid call record"));
        }
        self.conversation_row(&req.conversation_id)?;
        self.record_call_message(
            &req.conversation_id,
            &req.call_id,
            &req.caller_account_id,
            &req.status,
            req.duration_ms.max(0),
            req.started_at,
        )
    }

    /// Call history entry in the conversation (one per call; later records are ignored).
    fn record_call_message(&self, conv: &str, call_id: &str, caller: &str, status: &str, duration_ms: i64, at: i64) -> Result<()> {
        let value = json!({ "status": status, "duration_ms": duration_ms }).to_string();
        self.insert_system_message(conv, "call", Some(caller.to_string()), vec![], Some(value), Some(format!("call-{call_id}")), at)
    }

    /// ICE servers (STUN/TURN with short-lived credentials) from our server.
    pub async fn get_ice_servers(&self) -> Result<Value> {
        let s = self.session()?;
        s.api.get::<Value>("/v1/calls/ice").await
    }
}
