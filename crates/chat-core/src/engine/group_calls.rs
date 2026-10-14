//! Group voice and video calls: full mesh, no media server.
//!
//! Every pair of participants has its own WebRTC peer connection (DTLS-SRTP, the fingerprint
//! travels in the end-to-end encrypted SDP); media never touches our server except through the
//! TURN relay. That keeps calls small: at most [`MAX_PARTICIPANTS`] devices.
//!
//! Signaling reuses the call transport (`Content::Call`) with `group_id` set and kinds that
//! older clients do not know (they drop them instead of ringing a one-to-one call):
//! * `group_join` (to the whole group, sender keys): a device joins; `ring: true` from the one
//!   who starts the call rings the members. Participants repeat it every [`HEARTBEAT_MS`]
//!   (`heartbeat: true`, ephemeral) so crashed devices drop out. A participant that sees a new
//!   one answers with a targeted `group_join` so the newcomer learns who is there.
//! * `group_leave` (to the whole group): a device leaves.
//! * `group_decline` (to our own devices): stop ringing elsewhere.
//! * `group_offer` / `group_answer` / `group_ice` / `group_hangup` / `group_media` (pairwise,
//!   to one device): WebRTC signaling of one pair. To avoid glare, the device with the
//!   smaller `(account_id, device_id)` makes the offer; offers from the other side are dropped.
//!
//! Only end-to-end attested members (engine/attest.rs) take part: signals from anyone else are
//! dropped. Calls ring only in accepted conversations that are not muted, and not during do not
//! disturb (engine/dnd.rs), and never while calls are turned off (engine/call_settings.rs), which
//! also keeps this device from starting or joining one; otherwise the UI just shows that a call
//! is in progress. With relay-only calls every pair says `relay`. When the
//! last participant leaves, every member's history gets a "group call" entry.

use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::send::Job;
use super::{call_checks, Inner};
use crate::content::{CallSignal, Content};
use crate::error::{CoreError, ErrorCode, Result};
use crate::util::{new_id, now_ms};

/// Devices in one call (each pair is a peer connection, so this stays small).
pub const MAX_PARTICIPANTS: usize = 8;
/// A ring older than this (server time) does not ring anymore.
const RING_MAX_AGE_MS: i64 = 60_000;
/// Unanswered rings stop after this long.
const RING_TIMEOUT_MS: i64 = 45_000;
/// Participants repeat their presence this often...
const HEARTBEAT_MS: i64 = 20_000;
/// ...and are dropped after this long without it.
const PRESENCE_TIMEOUT_MS: i64 = 60_000;
/// Our own device leaves when the UI stopped confirming the call for this long (crashed or
/// reloaded webview).
const UI_TIMEOUT_MS: i64 = 60_000;
/// Other signals older than this belong to a call that is over.
const SIGNAL_MAX_AGE_MS: i64 = 120_000;
const SWEEP_EVERY: Duration = Duration::from_secs(5);

/// Pairwise kinds as the UI names them (sent as `group_<kind>`).
const PAIR_KINDS: &[&str] = &["offer", "answer", "ice", "hangup", "media"];

/// Whether the device `a` makes the offer to `b` (the smaller one does).
pub fn is_offerer(a: (&str, u32), b: (&str, u32)) -> bool {
    a < b
}

fn valid_call_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

#[derive(Debug, Clone)]
pub struct GroupCall {
    call_id: String,
    group_id: String,
    conversation_id: String,
    started_by: String,
    started_at: i64,
    video: bool,
    /// (account, device) -> last time we heard from it.
    participants: BTreeMap<(String, u32), i64>,
    /// This device is in the call.
    joined: bool,
    /// This device was in the call at some point (for the history entry).
    was_joined: bool,
    /// Last time the UI confirmed it is still in the call.
    ui_seen: i64,
    heartbeat_at: i64,
    ringing: bool,
    ring_at: i64,
    declined: bool,
}

#[derive(Debug, Serialize)]
pub struct GroupCallParticipant {
    pub account_id: String,
    pub device_id: u32,
    /// This very device.
    pub me: bool,
    /// We are not in contact with them: their pair must use the TURN relay only.
    pub relay: bool,
}

/// State of a group call (also the payload of the `group_call` event).
#[derive(Debug, Serialize)]
pub struct GroupCallInfo {
    pub group_id: String,
    pub conversation_id: String,
    pub call_id: String,
    pub started_by: String,
    pub started_at: i64,
    pub video: bool,
    pub participants: Vec<GroupCallParticipant>,
    pub joined: bool,
    /// This device should ring now.
    pub ringing: bool,
    /// The call is over (last event for it).
    pub ended: bool,
    pub max_participants: usize,
}

/// Payload of the `group_call_signal` event: WebRTC signaling of one pair.
#[derive(Debug, Serialize)]
pub struct GroupCallSignalEvent {
    pub group_id: String,
    pub conversation_id: String,
    pub call_id: String,
    pub account_id: String,
    pub device_id: u32,
    /// "offer" | "answer" | "ice" | "hangup" | "media"
    pub kind: String,
    pub data: Value,
    pub relay: bool,
}

#[derive(Debug, Deserialize)]
pub struct GroupCallStartRequest {
    pub conversation_id: String,
    #[serde(default)]
    pub video: bool,
}

#[derive(Debug, Deserialize)]
pub struct GroupCallJoinRequest {
    pub conversation_id: String,
    pub call_id: String,
    #[serde(default)]
    pub video: bool,
}

#[derive(Debug, Deserialize)]
pub struct GroupCallSignalRequest {
    pub conversation_id: String,
    pub call_id: String,
    pub account_id: String,
    pub device_id: u32,
    pub kind: String,
    #[serde(default)]
    pub data: Value,
}

impl GroupCall {
    fn new(call_id: &str, group_id: &str, conversation_id: &str, started_by: &str, started_at: i64, video: bool) -> Self {
        GroupCall {
            call_id: call_id.to_string(),
            group_id: group_id.to_string(),
            conversation_id: conversation_id.to_string(),
            started_by: started_by.to_string(),
            started_at,
            video,
            participants: BTreeMap::new(),
            joined: false,
            was_joined: false,
            ui_seen: 0,
            heartbeat_at: 0,
            ringing: false,
            ring_at: 0,
            declined: false,
        }
    }

    fn live(&self) -> bool {
        self.joined || !self.participants.is_empty()
    }
}

fn group_signal(group_id: &str, call_id: &str, kind: &str, data: Value, target_device: Option<u32>) -> Content {
    Content::Call(CallSignal {
        call_id: call_id.to_string(),
        kind: kind.to_string(),
        data,
        target_device,
        group_id: Some(group_id.to_string()),
    })
}

impl Inner {
    /// The group of a group conversation we are still in.
    fn call_group(&self, conv_id: &str) -> Result<String> {
        let row = self.conversation_row(conv_id)?;
        let gid = row.group_id.ok_or_else(|| CoreError::invalid("not a group conversation"))?;
        if self.group_left(&gid) {
            return Err(CoreError::new(ErrorCode::Forbidden, "you are not a member of this group"));
        }
        Ok(gid)
    }

    fn me_device(&self) -> Result<(String, u32)> {
        let s = self.session()?;
        Ok((s.account.account_id.clone(), s.account.device_id))
    }

    fn group_call_info(&self, c: &GroupCall, ended: bool) -> GroupCallInfo {
        let (me, dev) = self.me_device().unwrap_or_default();
        let relay_only = self.relay_only();
        GroupCallInfo {
            group_id: c.group_id.clone(),
            conversation_id: c.conversation_id.clone(),
            call_id: c.call_id.clone(),
            started_by: c.started_by.clone(),
            started_at: c.started_at,
            video: c.video,
            participants: c
                .participants
                .keys()
                .map(|(a, d)| GroupCallParticipant {
                    account_id: a.clone(),
                    device_id: *d,
                    me: *a == me && *d == dev,
                    relay: !(*a == me && *d == dev) && (relay_only || (*a != me && !self.is_contact(a))),
                })
                .collect(),
            joined: c.joined,
            ringing: c.ringing && !ended,
            ended,
            max_participants: MAX_PARTICIPANTS,
        }
    }

    fn emit_group_call(&self, group_id: &str) {
        let info = self.group_calls.borrow().get(group_id).map(|c| self.group_call_info(c, false));
        if let Some(info) = info {
            self.emit("group_call", &info);
        }
    }

    /// Ends the call once nobody is in it: history entry, last event.
    fn end_group_call_if_empty(&self, group_id: &str) {
        let ended = {
            let mut calls = self.group_calls.borrow_mut();
            match calls.get(group_id) {
                Some(c) if !c.live() => calls.remove(group_id),
                _ => None,
            }
        };
        let Some(c) = ended else { return };
        let value = json!({ "duration_ms": (now_ms() - c.started_at).max(0), "joined": c.was_joined }).to_string();
        if let Err(e) = self.insert_system_message(
            &c.conversation_id,
            "group_call",
            Some(c.started_by.clone()),
            vec![],
            Some(value),
            Some(format!("group-call-{}", c.call_id)),
            c.started_at,
        ) {
            log::warn!("group call history: {e}");
        }
        self.emit("group_call", &self.group_call_info(&c, true));
    }

    /// The call in progress in a group conversation, if any.
    pub fn group_call_state(&self, conv_id: &str) -> Result<Option<GroupCallInfo>> {
        let row = self.conversation_row(conv_id)?;
        let Some(gid) = row.group_id else { return Ok(None) };
        Ok(self.group_calls.borrow().get(&gid).filter(|c| c.live()).map(|c| self.group_call_info(c, false)))
    }

    /// Starts a call in the group (or joins the one already in progress) and rings the members.
    pub async fn group_call_start(self: &Rc<Self>, req: GroupCallStartRequest) -> Result<GroupCallInfo> {
        let gid = self.call_group(&req.conversation_id)?;
        let existing = self.group_calls.borrow().get(&gid).filter(|c| c.live()).map(|c| c.call_id.clone());
        if let Some(call_id) = existing {
            return self
                .group_call_join(GroupCallJoinRequest { conversation_id: req.conversation_id, call_id, video: req.video })
                .await;
        }
        let me = self.me()?;
        let call_id = new_id();
        let call = GroupCall::new(&call_id, &gid, &req.conversation_id, &me, now_ms(), req.video);
        self.group_calls.borrow_mut().insert(gid.clone(), call);
        self.join_group_call(&gid, &req.conversation_id, true, req.video)
    }

    /// Joins the call in progress in the group.
    pub async fn group_call_join(self: &Rc<Self>, req: GroupCallJoinRequest) -> Result<GroupCallInfo> {
        let gid = self.call_group(&req.conversation_id)?;
        let known = self.group_calls.borrow().get(&gid).is_some_and(|c| c.call_id == req.call_id && c.live());
        if !known {
            return Err(CoreError::not_found("this call is over"));
        }
        self.join_group_call(&gid, &req.conversation_id, false, req.video)
    }

    fn join_group_call(self: &Rc<Self>, gid: &str, conv_id: &str, ring: bool, video: bool) -> Result<GroupCallInfo> {
        if !self.calls_allowed() {
            return Err(CoreError::new(ErrorCode::Forbidden, "calls are turned off in settings"));
        }
        let me = self.me_device()?;
        let now = now_ms();
        let call_id = {
            let mut calls = self.group_calls.borrow_mut();
            let c = calls.get_mut(gid).ok_or_else(|| CoreError::not_found("this call is over"))?;
            let others = c.participants.keys().filter(|k| **k != me).count();
            if !c.joined && others >= MAX_PARTICIPANTS {
                return Err(CoreError::new(ErrorCode::Forbidden, "the call is full"));
            }
            c.joined = true;
            c.was_joined = true;
            c.ringing = false;
            c.ui_seen = now;
            c.heartbeat_at = now;
            c.participants.insert(me.clone(), now);
            c.call_id.clone()
        };
        // Calling into a group we were added to accepts it.
        self.accept_on_send(conv_id)?;
        self.enqueue(&Job::Group {
            group_id: gid.to_string(),
            content: group_signal(gid, &call_id, "group_join", json!({ "ring": ring, "video": video }), None),
        })?;
        self.emit_group_call(gid);
        let info = self.group_calls.borrow().get(gid).map(|c| self.group_call_info(c, false));
        info.ok_or_else(|| CoreError::not_found("this call is over"))
    }

    /// Leaves the call (the others are told; the call ends when the last one leaves).
    pub fn group_call_leave(&self, conv_id: &str) -> Result<()> {
        let row = self.conversation_row(conv_id)?;
        let Some(gid) = row.group_id else { return Ok(()) };
        self.leave_group_call(&gid, true)
    }

    fn leave_group_call(&self, gid: &str, tell: bool) -> Result<()> {
        let me = self.me_device()?;
        let call_id = {
            let mut calls = self.group_calls.borrow_mut();
            let Some(c) = calls.get_mut(gid) else { return Ok(()) };
            if !c.joined {
                return Ok(());
            }
            c.joined = false;
            c.participants.remove(&me);
            c.call_id.clone()
        };
        if tell && !self.group_left(gid) {
            self.enqueue(&Job::Group {
                group_id: gid.to_string(),
                content: group_signal(gid, &call_id, "group_leave", Value::Null, None),
            })?;
        }
        self.emit_group_call(gid);
        self.end_group_call_if_empty(gid);
        Ok(())
    }

    /// Stops ringing here and on our other devices.
    pub fn group_call_decline(&self, conv_id: &str) -> Result<()> {
        let gid = self.call_group(conv_id)?;
        let call_id = {
            let mut calls = self.group_calls.borrow_mut();
            let Some(c) = calls.get_mut(&gid) else { return Ok(()) };
            c.ringing = false;
            c.declined = true;
            c.call_id.clone()
        };
        self.enqueue(&Job::OwnDevices { content: group_signal(&gid, &call_id, "group_decline", Value::Null, None) })?;
        self.emit_group_call(&gid);
        Ok(())
    }

    /// The UI is still in the call (called every few seconds); without it this device leaves.
    pub fn group_call_heartbeat(&self, conv_id: &str) -> Result<()> {
        let row = self.conversation_row(conv_id)?;
        let Some(gid) = row.group_id else { return Ok(()) };
        if let Some(c) = self.group_calls.borrow_mut().get_mut(&gid).filter(|c| c.joined) {
            c.ui_seen = now_ms();
        }
        Ok(())
    }

    /// Pairwise WebRTC signaling to one participant of the call we are in.
    pub async fn group_call_signal(&self, req: GroupCallSignalRequest) -> Result<()> {
        if !PAIR_KINDS.contains(&req.kind.as_str()) || !valid_call_id(&req.call_id) {
            return Err(CoreError::invalid("invalid call signal"));
        }
        let gid = self.call_group(&req.conversation_id)?;
        let (me, dev) = self.me_device()?;
        let peer = (req.account_id.as_str(), req.device_id);
        if peer == (me.as_str(), dev) {
            return Err(CoreError::invalid("cannot signal ourselves"));
        }
        let in_call = self.group_calls.borrow().get(&gid).is_some_and(|c| c.joined && c.call_id == req.call_id);
        if !in_call {
            return Err(CoreError::not_found("not in this call"));
        }
        if !self.is_attested_member(&gid, &req.account_id) {
            return Err(CoreError::new(ErrorCode::Forbidden, "not a member of this group"));
        }
        let mine = (me.as_str(), dev);
        match req.kind.as_str() {
            "offer" if !is_offerer(mine, peer) => return Err(CoreError::invalid("the other device makes the offer")),
            "answer" if !is_offerer(peer, mine) => return Err(CoreError::invalid("the other device answers")),
            _ => {}
        }
        let mut data = req.data;
        if self.relay_only() {
            match req.kind.as_str() {
                "offer" | "answer" => data = call_checks::relay_only_sdp_signal(&data),
                "ice" => match call_checks::relay_only_ice_signal(&data) {
                    Some(d) => data = d,
                    None => return Ok(()),
                },
                _ => {}
            }
        }
        let content = group_signal(&gid, &req.call_id, &format!("group_{}", req.kind), data, Some(req.device_id));
        self.send_to_account(&req.account_id, &content, false).await
    }

    /// A group call signal from the network (`s.group_id` is set). `env_group` is the group of
    /// a sender-key envelope (None for pairwise ones).
    pub(super) async fn handle_group_call_signal(
        self: &Rc<Self>,
        from: &str,
        device: u32,
        sent_at: i64,
        env_group: Option<&str>,
        mut s: CallSignal,
    ) -> Result<()> {
        let Some(gid) = s.group_id.clone() else { return Ok(()) };
        let (me, my_dev) = self.me_device()?;
        if s.target_device.is_some_and(|d| d != my_dev) || (from == me && device == my_dev) {
            return Ok(());
        }
        if env_group.is_some_and(|g| g != gid) || !valid_call_id(&s.call_id) {
            return Ok(());
        }
        let from_me = from == me;
        // Only members an admin vouched for take part; never someone we blocked.
        if self.group_left(&gid) || !self.is_attested_member(&gid, from) || (!from_me && self.is_blocked(from)) {
            log::warn!("dropping group call signal from {from}: not a member of {gid}");
            return Ok(());
        }
        let Some(conv_id) = self.conversation_id_for_group(&gid)? else { return Ok(()) };
        let Some(age) = call_checks::signal_age(now_ms(), sent_at) else { return Ok(()) };
        let key = (from.to_string(), device);
        match s.kind.as_str() {
            "group_join" => {
                if age > PRESENCE_TIMEOUT_MS {
                    return Ok(());
                }
                let ring = s.data.get("ring").and_then(Value::as_bool).unwrap_or(false);
                let video = s.data.get("video").and_then(Value::as_bool).unwrap_or(false);
                let targeted = s.target_device.is_some();
                let known = self.group_calls.borrow().get(&gid).is_some_and(|c| c.call_id == s.call_id);
                // New calls in a group we did not accept count against the same per-caller limit
                // as one-to-one calls from strangers.
                if ring && !from_me && !known && !self.conversation_accepted_id(&conv_id) && !self.allow_call_offer(from, &s.call_id) {
                    log::warn!("dropping group call from {from}: too many calls");
                    return Ok(());
                }
                let now = now_ms();
                let mut replaced: Option<String> = None;
                let (new_participant, joined) = {
                    let mut calls = self.group_calls.borrow_mut();
                    match calls.get(&gid) {
                        Some(c) if c.call_id == s.call_id => {}
                        // Two calls started at once: both sides keep the smaller call id.
                        Some(c) if c.live() && c.call_id < s.call_id => return Ok(()),
                        other => {
                            if let Some(old) = other.filter(|c| c.joined) {
                                replaced = Some(old.call_id.clone());
                            }
                            calls.insert(gid.clone(), GroupCall::new(&s.call_id, &gid, &conv_id, from, sent_at, video));
                        }
                    }
                    let c = calls.get_mut(&gid).expect("inserted above");
                    let new = !c.participants.contains_key(&key);
                    if new && c.participants.len() >= MAX_PARTICIPANTS {
                        log::warn!("group call {} is full; ignoring {from}/{device}", c.call_id);
                        return Ok(());
                    }
                    c.participants.insert(key.clone(), now);
                    if from_me {
                        // Joined on another of our devices: stop ringing here.
                        c.ringing = false;
                    } else if ring && !c.joined && !c.declined && age <= RING_MAX_AGE_MS && self.call_rings_in(&conv_id) {
                        c.ringing = true;
                        c.ring_at = now;
                    }
                    (new, c.joined)
                };
                if let Some(old) = replaced {
                    // We were in the call that lost: leave it; the UI moves to the winner.
                    self.enqueue(&Job::Group { group_id: gid.clone(), content: group_signal(&gid, &old, "group_leave", Value::Null, None) })?;
                }
                if new_participant && joined && !targeted {
                    // Tell the newcomer we are here (it may have missed our join).
                    let reply = group_signal(&gid, &s.call_id, "group_join", json!({ "heartbeat": true }), Some(device));
                    let (inner, to) = (self.clone(), from.to_string());
                    tokio::task::spawn_local(async move {
                        if let Err(e) = inner.send_to_account(&to, &reply, false).await {
                            log::warn!("group call presence to {to}: {e}");
                        }
                    });
                }
                self.emit_group_call(&gid);
            }
            "group_leave" => {
                if age > SIGNAL_MAX_AGE_MS {
                    return Ok(());
                }
                let removed = {
                    let mut calls = self.group_calls.borrow_mut();
                    calls.get_mut(&gid).filter(|c| c.call_id == s.call_id).is_some_and(|c| c.participants.remove(&key).is_some())
                };
                if removed {
                    self.emit_group_call(&gid);
                    self.end_group_call_if_empty(&gid);
                }
            }
            "group_decline" => {
                if !from_me || age > SIGNAL_MAX_AGE_MS {
                    return Ok(());
                }
                let changed = {
                    let mut calls = self.group_calls.borrow_mut();
                    match calls.get_mut(&gid).filter(|c| c.call_id == s.call_id) {
                        Some(c) => {
                            c.declined = true;
                            std::mem::replace(&mut c.ringing, false)
                        }
                        None => false,
                    }
                };
                if changed {
                    self.emit_group_call(&gid);
                }
            }
            kind => {
                let Some(pair_kind) = kind.strip_prefix("group_").filter(|k| PAIR_KINDS.contains(k)) else { return Ok(()) };
                if s.target_device != Some(my_dev) || age > SIGNAL_MAX_AGE_MS {
                    return Ok(());
                }
                // Glare rule: offers only from the smaller device, answers from the larger.
                let ours = (me.as_str(), my_dev);
                let theirs = (from, device);
                match pair_kind {
                    "offer" if !is_offerer(theirs, ours) => {
                        log::warn!("dropping group call offer from {from}/{device}: we make that offer");
                        return Ok(());
                    }
                    "answer" if !is_offerer(ours, theirs) => return Ok(()),
                    "offer" | "answer" if !call_checks::valid_sdp_signal(&s.data) => return Ok(()),
                    "ice" => match call_checks::clean_ice_signal(&s.data) {
                        Some(d) => s.data = d,
                        None => return Ok(()),
                    },
                    _ => {}
                }
                {
                    let mut calls = self.group_calls.borrow_mut();
                    let Some(c) = calls.get_mut(&gid).filter(|c| c.joined && c.call_id == s.call_id) else { return Ok(()) };
                    if !c.participants.contains_key(&key) {
                        // Its join may still be on the way: an offer or answer counts it in
                        // if there is room; anything else from outside the call is dropped.
                        if !matches!(pair_kind, "offer" | "answer") || c.participants.len() >= MAX_PARTICIPANTS {
                            return Ok(());
                        }
                        c.participants.insert(key.clone(), now_ms());
                    }
                }
                self.emit(
                    "group_call_signal",
                    &GroupCallSignalEvent {
                        group_id: gid.clone(),
                        conversation_id: conv_id,
                        call_id: s.call_id,
                        account_id: from.to_string(),
                        device_id: device,
                        kind: pair_kind.to_string(),
                        data: s.data,
                        relay: self.relay_only() || (!from_me && !self.is_contact(from)),
                    },
                );
            }
        }
        Ok(())
    }

    /// Whether a group call rings here: the group is accepted (not a pending invite) and not
    /// muted, and do not disturb is off.
    fn call_rings_in(&self, conv_id: &str) -> bool {
        if self.dnd_active() || !self.calls_allowed() {
            return false;
        }
        let Ok(row) = self.conversation_row(conv_id) else { return false };
        let muted = row.muted_until.is_some_and(|m| m == -1 || m > now_ms());
        !muted && self.conversation_accepted(&row)
    }

    /// Expires rings, silent participants and our own presence; sends our heartbeats.
    pub(super) async fn group_call_loop(self: Rc<Self>) {
        loop {
            tokio::time::sleep(SWEEP_EVERY).await;
            self.sweep_group_calls().await;
        }
    }

    async fn sweep_group_calls(&self) {
        let Ok(me) = self.me_device() else { return };
        let now = now_ms();
        let gids: Vec<String> = self.group_calls.borrow().keys().cloned().collect();
        for gid in gids {
            if self.group_left(&gid) {
                // We are out of the group: the call is over for us.
                if let Some(c) = self.group_calls.borrow_mut().get_mut(&gid) {
                    c.joined = false;
                    c.participants.clear();
                }
                self.end_group_call_if_empty(&gid);
                continue;
            }
            let attested = self.attested(&gid);
            let mut leave = false;
            let mut heartbeat: Option<(String, bool)> = None;
            let changed = {
                let mut calls = self.group_calls.borrow_mut();
                let Some(c) = calls.get_mut(&gid) else { continue };
                let before = c.participants.len();
                c.participants.retain(|(a, d), seen| {
                    if (a.as_str(), *d) == (me.0.as_str(), me.1) {
                        return true;
                    }
                    // Removed from the group meanwhile, or silent too long.
                    let member = *a == me.0 || attested.as_ref().is_none_or(|x| x.is_member(a));
                    member && now - *seen <= PRESENCE_TIMEOUT_MS
                });
                let mut changed = c.participants.len() != before;
                if c.ringing && now - c.ring_at > RING_TIMEOUT_MS {
                    c.ringing = false;
                    changed = true;
                }
                if c.joined && now - c.ui_seen > UI_TIMEOUT_MS {
                    leave = true;
                } else if c.joined && now - c.heartbeat_at >= HEARTBEAT_MS {
                    c.heartbeat_at = now;
                    heartbeat = Some((c.call_id.clone(), c.video));
                }
                changed
            };
            if leave {
                log::warn!("leaving group call in {gid}: the UI stopped confirming it");
                if let Err(e) = self.leave_group_call(&gid, true) {
                    log::warn!("group call leave: {e}");
                }
                continue;
            }
            if changed {
                self.emit_group_call(&gid);
                self.end_group_call_if_empty(&gid);
            }
            if let Some((call_id, video)) = heartbeat {
                let content = group_signal(&gid, &call_id, "group_join", json!({ "heartbeat": true, "video": video }), None);
                if let Err(e) = self.send_to_group(&gid, &content, true).await {
                    log::debug!("group call heartbeat: {e}");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smaller_device_offers() {
        assert!(is_offerer(("a", 2), ("b", 1)));
        assert!(!is_offerer(("b", 1), ("a", 2)));
        assert!(is_offerer(("a", 1), ("a", 2)));
        assert!(!is_offerer(("a", 2), ("a", 1)));
        // Exactly one side of every pair offers.
        let devs = [("x", 1), ("x", 2), ("y", 1), ("z", 9)];
        for a in devs {
            for b in devs {
                if a != b {
                    assert_ne!(is_offerer(a, b), is_offerer(b, a));
                }
            }
        }
    }

    #[test]
    fn call_ids() {
        assert!(valid_call_id(&new_id()));
        assert!(!valid_call_id(""));
        assert!(!valid_call_id("a/b"));
        assert!(!valid_call_id(&"a".repeat(65)));
    }
}
