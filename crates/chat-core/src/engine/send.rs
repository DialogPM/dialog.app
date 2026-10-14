//! Encryption and delivery: outbox, 1:1 fan-out and Sender Key group sends.

use std::collections::{BTreeMap, HashSet};
use std::rc::Rc;
use std::str::FromStr;
use std::time::{Duration, SystemTime};

use libsignal_protocol::{
    create_sender_key_distribution_message, group_encrypt, kem, message_encrypt, process_prekey_bundle,
    DeviceId, IdentityKey, KyberPreKeyId, PreKeyBundle, PreKeyId, ProtocolAddress, PublicKey, SessionStore,
    SignedPreKeyId,
};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::Inner;
use crate::api::*;
use crate::content::{Content, DataMessage, SyncMessage};
use crate::crypto::{b64, unb64};
use crate::error::{CoreError, ErrorCode, Result};
use crate::util::now_ms;

/// A persistent unit of outgoing work.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "k", rename_all = "snake_case")]
pub enum Job {
    /// A stored message (uploads its attachments first).
    Message { message_id: String },
    /// A data message that modifies another one (edit, delete, reaction, timer update).
    Data { conversation_id: String, data: DataMessage },
    /// Control content to an account (all its devices).
    Account { account_id: String, content: Content },
    /// Control content to our own other devices.
    OwnDevices { content: Content },
    /// Control content to every member of a group.
    Group { group_id: String, content: Content },
    /// Finish a pending profile key rotation (engine/profile_key.rs).
    RotateProfileKey,
}

const MAX_ATTEMPTS: i64 = 8;

/// How long a message to an account with no device (logged out everywhere) keeps waiting in
/// the outbox before it is marked failed.
const NO_DEVICES_MAX_WAIT_MS: i64 = 7 * 24 * 3600 * 1000;

/// Retry delay while the recipient has no device, from how long the message has waited: a
/// quarter of that, between 15 s and 15 min. The outbox is also woken early on reconnect, on
/// our devices changing and when the peer shows up (`wake_no_device_jobs`).
fn no_devices_delay_ms(waited_ms: i64) -> i64 {
    (waited_ms / 4).clamp(15_000, 15 * 60 * 1000)
}

/// Maps a 404 "unknown account" from the keys or messages endpoints to `AccountGone`.
fn account_error(e: ApiErrorOrNet) -> CoreError {
    match e {
        ApiErrorOrNet::Api(a) if a.status == 404 && a.message == "unknown account" => {
            CoreError::new(ErrorCode::AccountGone, "this account no longer exists")
        }
        e => e.into(),
    }
}

impl Inner {
    pub fn enqueue(&self, job: &Job) -> Result<()> {
        self.db.borrow().conn.execute(
            "INSERT INTO outbox (job, attempts, next_attempt_at, created_at) VALUES (?1, 0, 0, ?2)",
            params![serde_json::to_string(job)?, now_ms()],
        )?;
        self.outbox_notify.notify_one();
        Ok(())
    }

    /// Makes the queued messages waiting for a recipient with no device (`status_reason`
    /// "no_devices") due now: on reconnect, when our devices change, when a peer comes online
    /// or after something reached them. `peer = None` wakes them all.
    pub fn wake_no_device_jobs(&self, peer: Option<&str>) {
        let woken = self.db.borrow().conn.execute(
            "UPDATE outbox SET next_attempt_at = 0 WHERE next_attempt_at > 0 AND (
               (json_extract(job, '$.k') = 'message' AND EXISTS (
                 SELECT 1 FROM messages m JOIN conversations c ON c.id = m.conversation_id
                  WHERE m.id = json_extract(outbox.job, '$.message_id')
                    AND m.status_reason = 'no_devices' AND m.status = 'pending'
                    AND (?1 IS NULL OR c.peer_account_id = ?1)))
               OR (?1 IS NOT NULL AND json_extract(job, '$.k') = 'data' AND EXISTS (
                 SELECT 1 FROM conversations c
                  WHERE c.id = json_extract(outbox.job, '$.conversation_id') AND c.peer_account_id = ?1)))",
            [peer],
        );
        match woken {
            Ok(0) => {}
            Ok(n) => {
                log::info!("outbox: retrying {n} message(s) waiting for a device");
                self.outbox_notify.notify_one();
            }
            Err(e) => log::warn!("outbox wake: {e}"),
        }
    }

    pub(super) async fn outbox_loop(self: Rc<Self>) {
        loop {
            let mut wait = Duration::from_secs(30);
            if self.ws_out.borrow().is_some() {
                match self.process_outbox().await {
                    Ok(Some(next)) => wait = next,
                    Ok(None) => {}
                    Err(e) => log::warn!("outbox: {e}"),
                }
            }
            let notify = self.outbox_notify.clone();
            tokio::select! {
                _ = notify.notified() => {}
                _ = tokio::time::sleep(wait) => {}
            }
        }
    }

    /// Processes due jobs in order. Returns the delay until the next retry, if any.
    async fn process_outbox(&self) -> Result<Option<Duration>> {
        loop {
            let now = now_ms();
            let next: Option<(i64, String, i64, i64)> = {
                let db = self.db.borrow();
                let mut st = db.conn.prepare(
                    "SELECT id, job, attempts, created_at FROM outbox WHERE next_attempt_at <= ?1 ORDER BY id LIMIT 1",
                )?;
                let mut rows = st.query([now])?;
                match rows.next()? {
                    Some(r) => Some((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                    None => None,
                }
            };
            let Some((id, job_json, attempts, created_at)) = next else {
                let pending: Option<i64> =
                    self.db.borrow().conn.query_row("SELECT MIN(next_attempt_at) FROM outbox", [], |r| r.get(0))?;
                return Ok(pending.map(|t| Duration::from_millis((t - now).max(100) as u64)));
            };
            let job: Job = match serde_json::from_str(&job_json) {
                Ok(j) => j,
                Err(e) => {
                    log::error!("dropping undecodable job: {e}");
                    self.db.borrow().conn.execute("DELETE FROM outbox WHERE id = ?1", [id])?;
                    continue;
                }
            };
            match self.run_job(&job).await {
                Ok(()) => {
                    self.db.borrow().conn.execute("DELETE FROM outbox WHERE id = ?1", [id])?;
                }
                Err(e) if e.code == ErrorCode::NoDevices && matches!(job, Job::Message { .. } | Job::Data { .. }) => {
                    // The peer logged out everywhere: keep the message pending (with the reason
                    // shown under it) until one of their devices is back, for up to a week.
                    // `attempts` is left alone: it counts real failures (MAX_ATTEMPTS).
                    let waited = now_ms() - created_at;
                    let expired = waited >= NO_DEVICES_MAX_WAIT_MS;
                    log::info!("job {id}: recipient has no devices{}", if expired { ", giving up" } else { "" });
                    if expired {
                        self.db.borrow().conn.execute("DELETE FROM outbox WHERE id = ?1", [id])?;
                    } else {
                        self.db.borrow().conn.execute(
                            "UPDATE outbox SET next_attempt_at = ?2 WHERE id = ?1",
                            params![id, now_ms() + no_devices_delay_ms(waited)],
                        )?;
                    }
                    if let Job::Message { message_id } = &job {
                        if expired {
                            self.set_status_reason(message_id, None)?;
                            self.set_message_status(message_id, "failed")?;
                        } else {
                            self.set_status_reason(message_id, Some("no_devices"))?;
                        }
                        // While waiting the reason is shown under the message itself; only
                        // giving up is an error worth reporting.
                        if expired {
                            self.emit(
                                "message_send_error",
                                &serde_json::json!({
                                    "message_id": message_id,
                                    "error": e.to_string(),
                                    "attempt": attempts + 1,
                                    "final": true,
                                }),
                            );
                        }
                    }
                }
                Err(e) if e.is_network() && self.conn_state.get() != super::net::ConnState::Online => {
                    // Offline: keep the order and wait for reconnection.
                    log::info!("outbox paused: {e}");
                    return Ok(Some(Duration::from_secs(15)));
                }
                Err(e) => {
                    // Includes network errors while the server is otherwise reachable (e.g. an
                    // upload the server or a proxy cuts off): retrying forever would leave the
                    // message pending with no explanation.
                    log::warn!("job {id} failed (attempt {}): {e}", attempts + 1);
                    let permanent = matches!(
                        e.code,
                        ErrorCode::NotFound
                            | ErrorCode::NoDevices
                            | ErrorCode::AccountGone
                            | ErrorCode::Forbidden
                            | ErrorCode::InvalidInput
                            | ErrorCode::PayloadTooLarge
                            | ErrorCode::UntrustedIdentity
                    );
                    let give_up = permanent || attempts + 1 >= MAX_ATTEMPTS;
                    if let Job::Message { message_id } = &job {
                        self.emit(
                            "message_send_error",
                            &serde_json::json!({
                                "message_id": message_id,
                                "error": e.to_string(),
                                "attempt": attempts + 1,
                                "final": give_up,
                            }),
                        );
                    }
                    if give_up {
                        self.db.borrow().conn.execute("DELETE FROM outbox WHERE id = ?1", [id])?;
                        if let Job::Message { message_id } = &job {
                            if e.code == ErrorCode::AccountGone {
                                self.set_status_reason(message_id, Some("account_gone"))?;
                            }
                            self.set_message_status(message_id, "failed")?;
                        }
                    } else {
                        let delay = 2i64.pow((attempts + 1) as u32).min(300) * 1000;
                        self.db.borrow().conn.execute(
                            "UPDATE outbox SET attempts = attempts + 1, next_attempt_at = ?2 WHERE id = ?1",
                            params![id, now_ms() + delay],
                        )?;
                    }
                }
            }
        }
    }

    async fn run_job(&self, job: &Job) -> Result<()> {
        match job {
            Job::Message { message_id } => {
                let Some((conversation_id, data)) = self.build_data_message(message_id).await? else {
                    return Ok(()); // message deleted meanwhile
                };
                self.deliver_data(&conversation_id, data)
                    .await
                    .map_err(|e| e.context("delivering the message (PUT /v1/messages)"))?;
                self.mark_sent(message_id)?;
                Ok(())
            }
            Job::Data { conversation_id, data } => self.deliver_data(conversation_id, data.clone()).await,
            Job::Account { account_id, content } => self.send_to_account(account_id, content, false).await,
            Job::Group { group_id, content } => self.send_to_group(group_id, content, false).await,
            Job::OwnDevices { content } => {
                let me = self.me()?;
                self.send_to_account(&me, content, false).await
            }
            Job::RotateProfileKey => self.rotate_profile_key().await,
        }
    }

    /// Sends a data message to a conversation (direct: peer + our other devices; group: all members).
    pub async fn deliver_data(&self, conversation_id: &str, mut data: DataMessage) -> Result<()> {
        let conv = self.conversation_row(conversation_id)?;
        // Our profile key only goes to accepted conversations (engine/requests.rs).
        let profile_key = self.profile_key_b64()?;
        // Not into a group with someone we blocked: they must not get a rotated key
        // (engine/profile_key.rs); the other members get it pairwise.
        let blocked_member = conv.group_id.as_deref().is_some_and(|g| self.group_has_blocked_member(g));
        data.profile_key = (self.conversation_accepted(&conv) && !blocked_member).then_some(profile_key);
        data.kt = self.kt_gossip_head();
        if let Some(group_id) = conv.group_id {
            data.group_id = Some(group_id.clone());
            self.send_to_group(&group_id, &Content::Data(data), false).await
        } else if let Some(peer) = conv.peer_account_id {
            if self.is_blocked(&peer) {
                return Err(CoreError::new(ErrorCode::Forbidden, "contact is blocked"));
            }
            // A peer with no device right now (NoDevices) still gets it later (the job stays
            // queued), but our own other devices get their copy now, once: a message already
            // marked "no_devices" had its copy sent on the first attempt.
            let to_peer = self.send_to_account(&peer, &Content::Data(data.clone()), false).await;
            if let Err(e) = &to_peer {
                if e.code != ErrorCode::NoDevices {
                    return to_peer;
                }
                let synced = self.message_row(&data.id)?.is_some_and(|m| m.status_reason.as_deref() == Some("no_devices"));
                if synced {
                    return to_peer;
                }
            }
            let me = self.me()?;
            if peer != me {
                let sync = Content::Sync(SyncMessage::Sent { destination: peer, message: data });
                self.send_to_account(&me, &sync, false).await?;
            }
            to_peer
        } else {
            Err(CoreError::internal("conversation has no target"))
        }
    }

    fn session_devices(&self, account_id: &str) -> Result<Vec<u32>> {
        let s = self.session()?;
        let db = self.db.borrow();
        let mut st = db.conn.prepare("SELECT device_id FROM sessions WHERE account_id = ?1 ORDER BY device_id")?;
        let devices: Vec<u32> = st.query_map([account_id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        Ok(devices
            .into_iter()
            .filter(|d| !(account_id == s.account.account_id && *d == s.account.device_id))
            .collect())
    }

    pub fn delete_session(&self, account_id: &str, device_id: u32) -> Result<()> {
        let db = self.db.borrow();
        db.conn.execute(
            "DELETE FROM sessions WHERE account_id = ?1 AND device_id = ?2",
            params![account_id, device_id],
        )?;
        // That device must receive our sender keys again.
        db.conn.execute(
            "DELETE FROM sender_key_shared WHERE account_id = ?1 AND device_id = ?2",
            params![account_id, device_id],
        )?;
        Ok(())
    }

    /// Fetches pre-key bundles and establishes sessions. `device = None` means all devices.
    pub async fn fetch_sessions(&self, account_id: &str, device: Option<u32>) -> Result<()> {
        let s = self.session()?;
        let device_part = match device {
            Some(d) => d.to_string(),
            None => "*".to_string(),
        };
        let path = format!("/v1/keys/{account_id}/{device_part}");
        // With the contact's access key, fetch anonymously (starting a session does not reveal
        // who is about to write to whom).
        let anonymous = match self.peer_access_key(account_id).filter(|_| account_id != s.account.account_id) {
            Some(key) => match s.api.get_sealed_keys(account_id, &device_part, &key).await {
                Ok(b) => Some(b),
                Err(ApiErrorOrNet::Api(e)) if e.status == 401 => None,
                Err(e) => return Err(account_error(e)),
            },
            None => None,
        };
        let bundles: BundlesDto = match anonymous {
            Some(b) => b,
            None => s.api.send_json::<(), _>(reqwest::Method::GET, &path, None).await.map_err(account_error)?,
        };
        let identity = IdentityKey::decode(&unb64(&bundles.identity_key)?)?;
        // Key transparency: a key we have not pinned yet must be the one the public log holds.
        self.kt_require(account_id, &identity.serialize()).await?;
        let local = s.local_address();
        for d in bundles.devices {
            if account_id == s.account.account_id && d.device_id == s.account.device_id {
                continue;
            }
            let device_id =
                DeviceId::new(d.device_id as u8).map_err(|_| CoreError::internal("invalid device id from server"))?;
            let prekey = match &d.prekey {
                Some(p) => Some((PreKeyId::from(p.id), PublicKey::deserialize(&unb64(&p.public_key)?)?)),
                None => None,
            };
            let bundle = PreKeyBundle::new(
                d.registration_id,
                device_id,
                prekey,
                SignedPreKeyId::from(d.signed_prekey.id),
                PublicKey::deserialize(&unb64(&d.signed_prekey.public_key)?)?,
                unb64(&d.signed_prekey.signature)?,
                KyberPreKeyId::from(d.kyber_prekey.id),
                kem::PublicKey::deserialize(&unb64(&d.kyber_prekey.public_key)?)?,
                unb64(&d.kyber_prekey.signature)?,
                identity,
            )?;
            let address = ProtocolAddress::new(account_id.to_string(), device_id);
            let mut sessions = self.signal_store()?;
            let mut identities = self.signal_store()?;
            if let Err(e) = process_prekey_bundle(
                &address,
                &local,
                &mut sessions,
                &mut identities,
                &bundle,
                SystemTime::now(),
                &mut rand::rng(),
            )
            .await
            {
                if matches!(e, libsignal_protocol::SignalProtocolError::UntrustedIdentity(_)) {
                    // A device that does not belong to the account (pinned identity): refuse to
                    // send to the account at all rather than leak messages to it.
                    self.process_identity_conflicts()?;
                    return Err(e.into());
                }
                // A bad signature means the server served keys not signed by the account identity.
                log::error!("rejecting bundle for {address}: {e}");
                continue;
            }
            self.delete_sender_key_shared(account_id, d.device_id)?;
        }
        self.process_identity_conflicts()?;
        Ok(())
    }

    fn delete_sender_key_shared(&self, account_id: &str, device_id: u32) -> Result<()> {
        self.db.borrow().conn.execute(
            "DELETE FROM sender_key_shared WHERE account_id = ?1 AND device_id = ?2",
            params![account_id, device_id],
        )?;
        Ok(())
    }

    async fn encrypt_for(&self, account_id: &str, devices: &[u32], plaintext: &[u8]) -> Result<Vec<OutgoingMessageDto>> {
        let s = self.session()?;
        let local = s.local_address();
        let mut out = Vec::with_capacity(devices.len());
        for &d in devices {
            let address = ProtocolAddress::new(
                account_id.to_string(),
                DeviceId::new(d as u8).map_err(|_| CoreError::internal("invalid device id"))?,
            );
            let mut sessions = self.signal_store()?;
            let mut identities = self.signal_store()?;
            let msg = message_encrypt(
                plaintext,
                &address,
                &local,
                &mut sessions,
                &mut identities,
                SystemTime::now(),
                &mut rand::rng(),
            )
            .await?;
            let registration_id = sessions
                .load_session(&address)
                .await?
                .ok_or_else(|| CoreError::internal("session vanished"))?
                .remote_registration_id()?;
            out.push(OutgoingMessageDto {
                device_id: d,
                registration_id,
                kind: msg.message_type() as u8,
                content: b64(msg.serialize()),
            });
        }
        Ok(out)
    }

    /// Encrypts `content` for every device of `account_id` and sends it.
    /// For our own account, it goes to our other devices only.
    pub async fn send_to_account(&self, account_id: &str, content: &Content, online: bool) -> Result<()> {
        let _guard = self.send_lock.lock().await;
        let s = self.session()?;
        let is_self = account_id == s.account.account_id;
        let plaintext = content.encode();
        // A call offer to someone else asks the server for a call push (high priority, CallKit
        // on iOS) for the callee's devices that need one.
        let call_push = match content {
            Content::Call(c) if c.kind == "offer" && !is_self && !online => Some(c.call_id.clone()),
            _ => None,
        };
        let mut fetched_all = false;
        // Sealed sender when we know the contact's access key (never for our own devices).
        let mut sealed = match (is_self, self.peer_access_key(account_id)) {
            (false, Some(key)) => self.sender_certificate().await.map(|cert| (key, cert)),
            _ => None,
        };
        for _ in 0..6 {
            let mut devices = self.session_devices(account_id)?;
            if devices.is_empty() && !is_self {
                if online {
                    return Ok(()); // never consume pre-keys for ephemeral messages
                }
                if fetched_all {
                    return Err(CoreError::new(ErrorCode::NoDevices, "recipient has no devices"));
                }
                self.fetch_sessions(account_id, None).await?;
                fetched_all = true;
                devices = self.session_devices(account_id)?;
                if devices.is_empty() {
                    return Err(CoreError::new(ErrorCode::NoDevices, "recipient has no devices"));
                }
            }
            if is_self && devices.is_empty() && self.no_other_devices.get() {
                return Ok(());
            }
            let messages = match &sealed {
                Some((_, cert)) => self.seal_for(account_id, &devices, &plaintext, cert).await?,
                None => self.encrypt_for(account_id, &devices, &plaintext).await?,
            };
            let req = SendReq { timestamp: content_timestamp(content), online, messages, call_id: call_push.clone() };
            let sent = match &sealed {
                Some((key, _)) => s.api.put_sealed_messages(account_id, key, &req).await,
                None => s.api.put_messages(account_id, &req).await,
            };
            match sent {
                Ok(_) => {
                    if is_self {
                        self.no_other_devices.set(devices.is_empty());
                    } else if !online {
                        // They have a device again: older messages still waiting for one go
                        // next, in order.
                        self.wake_no_device_jobs(Some(account_id));
                    }
                    return Ok(());
                }
                Err(ApiErrorOrNet::Api(e)) if e.status == 401 && sealed.is_some() => {
                    // Their access key changed (or they have none yet): send identified.
                    log::info!("sealed delivery to {account_id} refused; sending identified");
                    sealed = None;
                }
                Err(ApiErrorOrNet::Api(e)) if e.status == 409 => {
                    let missing: Vec<u32> = serde_json::from_value(e.body["missing_devices"].clone()).unwrap_or_default();
                    let extra: Vec<u32> = serde_json::from_value(e.body["extra_devices"].clone()).unwrap_or_default();
                    for d in extra {
                        self.delete_session(account_id, d)?;
                    }
                    for d in missing {
                        if online {
                            continue;
                        }
                        self.fetch_sessions(account_id, Some(d)).await?;
                    }
                    if online {
                        return Ok(());
                    }
                }
                Err(ApiErrorOrNet::Api(e)) if e.status == 410 => {
                    let stale: Vec<u32> = serde_json::from_value(e.body["stale_devices"].clone()).unwrap_or_default();
                    for d in stale {
                        self.delete_session(account_id, d)?;
                        if !online {
                            self.fetch_sessions(account_id, Some(d)).await?;
                        }
                    }
                    if online {
                        return Ok(());
                    }
                }
                Err(e) => return Err(account_error(e)),
            }
        }
        Err(CoreError::internal("device list kept changing"))
    }

    pub async fn group_recipients(&self, group_id: &str, refresh: bool) -> Result<Vec<RecipientDto>> {
        if !refresh {
            if let Some(r) = self.group_devices.borrow().get(group_id) {
                return Ok(r.clone());
            }
        }
        if self.is_group_v2(group_id) {
            let list = self.group_recipients_v2(group_id).await?;
            self.group_devices.borrow_mut().insert(group_id.to_string(), list.clone());
            return Ok(list);
        }
        let s = self.session()?;
        #[derive(Deserialize)]
        struct Resp {
            devices: Vec<RecipientDto>,
        }
        let r: Resp = s.api.get(&format!("/v1/groups/{group_id}/devices")).await?;
        let list: Vec<RecipientDto> = r
            .devices
            .into_iter()
            .filter(|d| !(d.account_id == s.account.account_id && d.device_id == s.account.device_id))
            .collect();
        // Devices of accounts we do not know as members: the server changed the group without
        // telling us (possibly adding an account of its own). Re-sync so it shows up, flagged
        // if no admin confirmed it.
        let known: std::collections::HashSet<String> = self.group_member_ids(group_id)?.into_iter().collect();
        if list.iter().any(|d| !known.contains(&d.account_id)) {
            Box::pin(self.sync_group(group_id, None)).await?;
        }
        self.group_devices.borrow_mut().insert(group_id.to_string(), list.clone());
        Ok(list)
    }

    fn distribution_id(&self, group_id: &str) -> Result<Uuid> {
        let d: String = self.db.borrow().conn.query_row(
            "SELECT distribution_id FROM groups WHERE group_id = ?1",
            [group_id],
            |r| r.get(0),
        )?;
        Uuid::from_str(&d).map_err(|e| CoreError::internal(e.to_string()))
    }

    /// Makes sure every recipient device has our current sender key.
    async fn distribute_sender_key(&self, group_id: &str, dist: Uuid, recipients: &[RecipientDto]) -> Result<bool> {
        let shared: HashSet<(String, u32)> = {
            let db = self.db.borrow();
            let mut st = db
                .conn
                .prepare("SELECT account_id, device_id FROM sender_key_shared WHERE distribution_id = ?1")?;
            let rows = st.query_map([dist.to_string()], |r| Ok((r.get::<_, String>(0)?, r.get::<_, u32>(1)?)))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let mut by_account: BTreeMap<String, Vec<u32>> = BTreeMap::new();
        // Our sender key goes only to members an admin confirmed end to end (and to our own
        // devices). A device the server added on its own still receives the group ciphertext
        // (the server requires the full fan-out) but can never decrypt it.
        for r in recipients.iter().filter(|r| self.is_attested_member(group_id, &r.account_id)) {
            by_account.entry(r.account_id.clone()).or_default().push(r.device_id);
        }
        let needing: Vec<(String, Vec<u32>)> = by_account
            .into_iter()
            .filter(|(a, ds)| ds.iter().any(|d| !shared.contains(&(a.clone(), *d))))
            .collect();
        if needing.is_empty() {
            return Ok(true);
        }
        let s = self.session()?;
        let mut store = self.signal_store()?;
        let skdm = create_sender_key_distribution_message(&s.local_address(), dist, &mut store, &mut rand::rng()).await?;
        let content = Content::SenderKey { group_id: group_id.to_string(), skdm: b64(skdm.serialized()) };
        for (account, devices) in needing {
            match self.send_to_account(&account, &content, false).await {
                Ok(()) => {
                    // send_to_account may have discovered more devices; mark what we have sessions for.
                    let mut all = self.session_devices(&account)?;
                    all.extend(devices);
                    let db = self.db.borrow();
                    for d in all {
                        db.conn.execute(
                            "INSERT OR IGNORE INTO sender_key_shared (distribution_id, account_id, device_id) VALUES (?1, ?2, ?3)",
                            params![dist.to_string(), account, d],
                        )?;
                    }
                }
                Err(e) if matches!(e.code, ErrorCode::NotFound | ErrorCode::NoDevices | ErrorCode::AccountGone) => {
                    log::warn!("skipping sender key for {account}: {e}");
                }
                Err(e) => return Err(e),
            }
        }
        Ok(false)
    }

    pub async fn send_to_group(&self, group_id: &str, content: &Content, online: bool) -> Result<()> {
        let left: bool = self
            .db
            .borrow()
            .conn
            .query_row("SELECT left_group FROM groups WHERE group_id = ?1", [group_id], |r| r.get(0))
            .map_err(|_| CoreError::not_found("unknown group"))?;
        if left {
            return Err(CoreError::new(ErrorCode::Forbidden, "you are not a member of this group"));
        }
        let plaintext = content.encode();
        let mut refresh = false;
        for _ in 0..5 {
            let dist = self.distribution_id(group_id)?;
            let recipients = self.group_recipients(group_id, refresh).await?;
            if recipients.is_empty() {
                return Ok(()); // alone in the group
            }
            if online {
                // Ephemeral messages never trigger key distribution.
                let shared: i64 = self.db.borrow().conn.query_row(
                    "SELECT COUNT(*) FROM sender_key_shared WHERE distribution_id = ?1",
                    [dist.to_string()],
                    |r| r.get(0),
                )?;
                if (shared as usize) < recipients.len() {
                    return Ok(());
                }
            } else {
                self.distribute_sender_key(group_id, dist, &recipients).await?;
            }
            let s = self.session()?;
            let skm = {
                let _guard = self.send_lock.lock().await;
                let mut store = self.signal_store()?;
                group_encrypt(&mut store, &s.local_address(), dist, &plaintext, &mut rand::rng()).await?
            };
            let req = GroupSendReq {
                timestamp: content_timestamp(content),
                online,
                content: b64(skm.serialized()),
                recipients: recipients.clone(),
            };
            match self.put_group_messages_any(group_id, &req).await {
                Ok(_) => return Ok(()),
                Err(ApiErrorOrNet::Api(e)) if e.status == 409 || e.status == 410 => {
                    if let Ok(stale) = serde_json::from_value::<Vec<RecipientDto>>(e.body["stale"].clone()) {
                        for d in stale {
                            self.delete_session(&d.account_id, d.device_id)?;
                        }
                    }
                    refresh = true;
                    if online {
                        let _ = self.group_recipients(group_id, true).await;
                        return Ok(());
                    }
                }
                Err(ApiErrorOrNet::Api(e)) if e.status == 403 => {
                    let _ = self.sync_group(group_id, None).await;
                    return Err(CoreError::new(ErrorCode::Forbidden, "not allowed to send to this group"));
                }
                Err(e) => return Err(e.into()),
            }
        }
        Err(CoreError::internal("group membership kept changing"))
    }

    /// Sends typing indicators (best effort, never queued).
    pub async fn send_typing(&self, conversation_id: &str, typing: bool) -> Result<()> {
        if !self.privacy()?.typing_indicators {
            return Ok(());
        }
        let conv = self.conversation_row(conversation_id)?;
        if !self.conversation_accepted(&conv) {
            return Ok(()); // message request / pending group invite
        }
        if let Some(g) = conv.group_id {
            let content = Content::Typing { typing, group_id: Some(g.clone()) };
            self.send_to_group(&g, &content, true).await
        } else if let Some(peer) = conv.peer_account_id {
            if self.is_blocked(&peer) || peer == self.me()? {
                return Ok(());
            }
            self.send_to_account(&peer, &Content::Typing { typing, group_id: None }, true).await
        } else {
            Ok(())
        }
    }
}

/// Envelope timestamp: for data messages it equals `sent_at`, so that a recipient that fails to
/// decrypt can ask for exactly that message again (see `ResendRequest`).
fn content_timestamp(content: &Content) -> i64 {
    match content {
        Content::Data(d) => d.sent_at,
        Content::Sync(SyncMessage::Sent { message, .. }) => message.sent_at,
        _ => now_ms(),
    }
}
