//! Encrypted server vault: syncs contacts, group keys, conversation settings and (optionally)
//! message history between the devices of an account. Records are encrypted with the vault
//! key derived from the account secret; record ids are HMACs of logical ids.
//!
//! The server stores the vault and is not trusted with it: it cannot read or forge a record,
//! but it could hand out an older copy, or one record in place of another. Against that:
//! * every record carries a version `(n, d)`: a counter that only grows (one more than the
//!   highest this device has seen, a Lamport clock) and the id of the device that wrote it.
//!   Version and record id are authenticated with the content. A device keeps the newest
//!   version of each record it wrote or applied (table `vault_versions`) and refuses anything
//!   below it; it then uploads its own copy again, so the vault heals.
//! * a device that links another one writes a manifest (the versions of every record except
//!   messages) and sends its hash inside the link payload, end to end. The new device takes
//!   those versions as the minimum before it applies anything, and asks for a new upload
//!   when the server's vault falls short.
//! * signing in with the password or the recovery phrase has no such anchor: the profile key
//!   is rotated once the restore is done, so a key the server brought back is not kept.
//! Two devices changing the same record: the higher version wins (the device id breaks a tie)
//! and the per-record rules of `apply_record` still merge what they can (accepted chats,
//! message edits and deletions, timestamps of settings).

use std::rc::Rc;
use std::time::Duration;

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::messages::{MessageRow, NewMessage};
use super::Inner;
use crate::api::{VaultPageDto, VaultRecordDto};
use crate::content::{Content, SyncMessage};
use crate::crypto::{self, b64, unb64};
use crate::error::Result;
use crate::model::*;

/// Pages of 500 records one pull applies before yielding to the next sync.
const MAX_VAULT_PAGES_PER_PULL: usize = 200;
use crate::util::now_ms;

/// Records written before versions existed (no header; they count as version (0, 0)).
const VAULT_AAD: &[u8] = b"chatapp-vault-v1";
const VAULT_AAD_V2: &[u8] = b"chatapp-vault-v2";
const BLOB_V2: u8 = 2;
const HEADER_LEN: usize = 13;
const BATCH: usize = 200;
/// Entries per manifest record (a record holds at most 256 KiB).
const MANIFEST_PART: usize = 2000;
/// How long a newly linked device waits for the manifest before it goes on without one.
const FLOOR_WAIT_MS: i64 = 120_000;
/// Pulls, 10 s apart, before a started engine settles for the slow rhythm.
const SETTLING_PULLS: u32 = 6;
/// A record the server returned older than known is uploaded again at most this often.
const HEAL_EVERY_MS: i64 = 600_000;

/// Version of a record: (counter, device id), compared in that order.
type Version = (u64, u32);
/// (record id, counter, device id)
type ManifestEntry = (String, u64, u32);

/// What a linking device vouches for: the manifest records and the hash of their content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultFloor {
    pub parts: u32,
    pub hash: String,
}

struct Known {
    version: Version,
    lid: Option<String>,
    applied: bool,
}

fn aad_v2(record_id: &str, header: &[u8]) -> Vec<u8> {
    [VAULT_AAD_V2, &[0u8], record_id.as_bytes(), &[0u8], header].concat()
}

/// `[2][n: u64 BE][d: u32 BE]` followed by the sealed content; the header and the record id
/// are authenticated with it.
fn seal_record(key: &[u8; 32], record_id: &str, version: Version, plain: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(HEADER_LEN + plain.len() + 28);
    out.push(BLOB_V2);
    out.extend_from_slice(&version.0.to_be_bytes());
    out.extend_from_slice(&version.1.to_be_bytes());
    let sealed = crypto::aead_seal(key, plain, &aad_v2(record_id, &out))?;
    out.extend_from_slice(&sealed);
    Ok(out)
}

fn open_record(key: &[u8; 32], record_id: &str, blob: &[u8]) -> Result<(Version, Vec<u8>)> {
    if blob.len() > HEADER_LEN && blob[0] == BLOB_V2 {
        let (header, sealed) = blob.split_at(HEADER_LEN);
        if let Ok(plain) = crypto::aead_open(key, sealed, &aad_v2(record_id, header)) {
            let n = u64::from_be_bytes(header[1..9].try_into().expect("8 bytes"));
            let d = u32::from_be_bytes(header[9..13].try_into().expect("4 bytes"));
            return Ok(((n, d), plain));
        }
    }
    Ok(((0, 0), crypto::aead_open(key, blob, VAULT_AAD)?))
}

/// Where a conversation points to (stable across devices, unlike local conversation ids).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
enum Target {
    Peer { account_id: String },
    Group { group_id: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "r", rename_all = "snake_case")]
enum Record {
    Deleted {
        lid: String,
    },
    Settings {
        privacy: PrivacySettings,
        /// Our rotated profile key (engine/profile_key.rs), so new devices use it too.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        profile_key: Option<String>,
        /// Do not disturb (engine/dnd.rs); absent in records from older versions.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        dnd: Option<super::dnd::DndSetting>,
        /// Call privacy settings (engine/call_settings.rs); absent in records from older versions.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        calls: Option<super::call_settings::CallSettingsRecord>,
        /// Media retention period (engine/retention.rs); absent in records from older versions.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        media_retention: Option<super::retention::MediaRetentionRecord>,
        /// Default disappearing timer (engine/default_timer.rs); absent in records from older versions.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        default_timer: Option<super::default_timer::DefaultTimerRecord>,
    },
    Contact {
        account_id: String,
        number: String,
        local_name: Option<String>,
        phone: Option<String>,
        blocked: bool,
        verified: bool,
        is_contact: bool,
        profile_key: Option<String>,
        updated_at: i64,
        /// Pinned Signal identity key (base64), so a newly linked device starts with the
        /// identities this account already trusts instead of trusting the server.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        identity_key: Option<String>,
    },
    Group {
        group_id: String,
        group_key: String,
        /// End-to-end attested member list (engine/attest.rs), so a newly linked device does
        /// not have to take the server's list.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        attested: Option<super::attest::Attested>,
    },
    Conversation {
        target: Target,
        muted_until: Option<i64>,
        archived: bool,
        pinned: bool,
        disappearing_seconds: i64,
        /// Message request accepted (records from older versions: everything was accepted).
        #[serde(default = "accepted_default")]
        accepted: bool,
    },
    Message(Box<VaultMessage>),
    /// Versions of the records (messages aside) as a device knew them when it linked another.
    Manifest {
        part: u32,
        entries: Vec<ManifestEntry>,
    },
}

/// The logical id a record belongs under (its record id is derived from it).
fn lid_of(rec: &Record) -> String {
    let target = |t: &Target| match t {
        Target::Peer { account_id } => format!("conv:peer:{account_id}"),
        Target::Group { group_id } => format!("conv:group:{group_id}"),
    };
    match rec {
        Record::Deleted { lid } => lid.clone(),
        Record::Settings { .. } => "settings".into(),
        Record::Contact { account_id, .. } => format!("contact:{account_id}"),
        Record::Group { group_id, .. } => format!("group:{group_id}"),
        Record::Conversation { target: t, .. } => target(t),
        Record::Message(m) => format!("msg:{}", m.id),
        Record::Manifest { part, .. } => format!("manifest:{part}"),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct VaultMessage {
    id: String,
    target: Target,
    sender_account_id: String,
    outgoing: bool,
    body: Option<String>,
    attachments: Vec<StoredAttachment>,
    location: Option<Location>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    contacts: Vec<SharedContact>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    mentions: Vec<Mention>,
    reply_to_id: Option<String>,
    forwarded: bool,
    status: String,
    sent_at: i64,
    received_at: i64,
    edited_at: Option<i64>,
    deleted: bool,
    system: Option<StoredSystemEvent>,
    reactions: Vec<(String, String)>,
    read: bool,
}

impl Inner {
    fn mark_dirty(&self, lid: &str) {
        let _ = self
            .db
            .borrow()
            .conn
            .execute("INSERT OR IGNORE INTO vault_dirty (logical_id) VALUES (?1)", [lid]);
        self.vault_notify.notify_one();
    }

    pub fn vault_put_message(&self, id: &str) {
        if !self.backup_enabled() || self.temporary_mode() {
            return;
        }
        // Disappearing messages are never backed up (and must not produce tombstones either,
        // which would delete them on every device, including this one).
        if matches!(self.message_row(id), Ok(Some(m)) if m.is_disappearing()) {
            return;
        }
        self.mark_dirty(&format!("msg:{id}"));
    }

    pub fn vault_put_conversation(&self, conv_id: &str) {
        if self.temporary_mode() {
            return;
        }
        if let Ok(row) = self.conversation_row(conv_id) {
            if let Some(p) = row.peer_account_id {
                self.mark_dirty(&format!("conv:peer:{p}"));
            } else if let Some(g) = row.group_id {
                self.mark_dirty(&format!("conv:group:{g}"));
            }
        }
    }

    pub fn vault_put_contact(&self, account_id: &str) {
        self.mark_dirty(&format!("contact:{account_id}"));
    }

    pub fn vault_put_group(&self, group_id: &str) {
        self.mark_dirty(&format!("group:{group_id}"));
    }

    pub async fn vault_put_settings(&self) {
        self.mark_dirty("settings");
    }

    /// Marks a logical record as deleted (the flush will write a tombstone).
    pub fn vault_delete(&self, lid: &str) {
        self.mark_dirty(lid);
    }

    pub fn vault_backfill_messages(&self) -> Result<()> {
        self.db.borrow().conn.execute(
            "INSERT OR IGNORE INTO vault_dirty (logical_id) SELECT 'msg:' || id FROM messages WHERE expires_at IS NULL AND expires_in = 0 AND view_once = 0",
            [],
        )?;
        Ok(())
    }

    fn target_of(&self, conv_id: &str) -> Option<Target> {
        let row = self.conversation_row(conv_id).ok()?;
        match (row.peer_account_id, row.group_id) {
            (Some(p), _) => Some(Target::Peer { account_id: p }),
            (None, Some(g)) => Some(Target::Group { group_id: g }),
            _ => None,
        }
    }

    /// True for a disappearing message that still exists locally.
    fn is_ephemeral_message(&self, lid: &str) -> Result<bool> {
        Ok(match lid.strip_prefix("msg:") {
            Some(id) => matches!(self.message_row(id)?, Some(m) if m.is_disappearing()),
            None => false,
        })
    }

    /// Builds the current record for a logical id (None = nothing to store, tombstone).
    fn build_record(&self, lid: &str) -> Result<Option<Record>> {
        let db = self.db.borrow();
        if let Some(part) = lid.strip_prefix("manifest:") {
            let parts: Vec<Vec<ManifestEntry>> = db.kv_get_json("vault_manifest")?.unwrap_or_default();
            let part = part.parse::<usize>().ok();
            return Ok(part.and_then(|i| parts.get(i).cloned().map(|entries| Record::Manifest { part: i as u32, entries })));
        }
        if lid == "settings" {
            drop(db);
            return Ok(Some(Record::Settings {
                privacy: self.privacy()?,
                profile_key: self.rotated_profile_key_b64(),
                dnd: Some(self.dnd_setting()).filter(|d| d.updated_at > 0),
                calls: Some(self.call_settings_record()).filter(|c| c.updated_at > 0),
                media_retention: Some(self.media_retention_record()).filter(|r| r.updated_at > 0),
                default_timer: Some(self.default_timer_record()).filter(|r| r.updated_at > 0),
            }));
        }
        if let Some(id) = lid.strip_prefix("contact:") {
            let r = db
                .conn
                .query_row(
                    "SELECT number, local_name, phone, blocked, verified, is_contact, profile_key, updated_at FROM contacts WHERE account_id = ?1",
                    [id],
                    |r| {
                        Ok(Record::Contact {
                            account_id: id.to_string(),
                            number: r.get(0)?,
                            local_name: r.get(1)?,
                            phone: r.get(2)?,
                            blocked: r.get(3)?,
                            verified: r.get(4)?,
                            is_contact: r.get(5)?,
                            profile_key: r.get::<_, Option<Vec<u8>>>(6)?.map(b64),
                            updated_at: r.get(7)?,
                            identity_key: None,
                        })
                    },
                )
                .optional()?;
            let identity: Option<Vec<u8>> = db
                .conn
                .query_row("SELECT identity_key FROM identities WHERE account_id = ?1", [id], |r| r.get(0))
                .optional()?;
            return Ok(r.map(|rec| match rec {
                Record::Contact { identity_key: _, account_id, number, local_name, phone, blocked, verified, is_contact, profile_key, updated_at } => {
                    Record::Contact { identity_key: identity.map(b64), account_id, number, local_name, phone, blocked, verified, is_contact, profile_key, updated_at }
                }
                other => other,
            }));
        }
        if let Some(gid) = lid.strip_prefix("group:") {
            let key: Option<Option<Vec<u8>>> = db
                .conn
                .query_row("SELECT group_key FROM groups WHERE group_id = ?1", [gid], |r| r.get(0))
                .optional()?;
            let attested = db.kv_get_json::<super::attest::Attested>(&format!("group_attested:{gid}")).ok().flatten();
            return Ok(key.flatten().map(|k| Record::Group { group_id: gid.to_string(), group_key: b64(k), attested }));
        }
        if let Some(rest) = lid.strip_prefix("conv:") {
            let (col, target) = if let Some(p) = rest.strip_prefix("peer:") {
                ("peer_account_id", Target::Peer { account_id: p.to_string() })
            } else if let Some(g) = rest.strip_prefix("group:") {
                ("group_id", Target::Group { group_id: g.to_string() })
            } else {
                return Ok(None);
            };
            let key = rest.split_once(':').map(|(_, k)| k).unwrap_or_default();
            let r = db
                .conn
                .query_row(
                    &format!("SELECT muted_until, archived, pinned, disappearing_seconds, accepted FROM conversations WHERE {col} = ?1"),
                    [key],
                    |r| {
                        Ok(Record::Conversation {
                            target: target.clone(),
                            muted_until: r.get(0)?,
                            archived: r.get(1)?,
                            pinned: r.get(2)?,
                            disappearing_seconds: r.get(3)?,
                            accepted: r.get(4)?,
                        })
                    },
                )
                .optional()?;
            return Ok(r);
        }
        if let Some(id) = lid.strip_prefix("msg:") {
            drop(db);
            let Some(m) = self.message_row(id)? else { return Ok(None) };
            if m.is_disappearing() {
                return Ok(None); // disappearing messages are never backed up
            }
            let Some(target) = self.target_of(&m.conversation_id) else { return Ok(None) };
            let reactions: Vec<(String, String)> = self
                .db
                .borrow()
                .conn
                .prepare("SELECT account_id, emoji FROM reactions WHERE message_id = ?1")?
                .query_map([id], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?;
            let read: bool = self
                .db
                .borrow()
                .conn
                .query_row("SELECT read_local FROM messages WHERE id = ?1", [id], |r| r.get(0))?;
            return Ok(Some(Record::Message(Box::new(vault_message(m, target, reactions, read)))));
        }
        Ok(None)
    }

    async fn vault_flush(&self) -> Result<()> {
        let s = self.session()?;
        loop {
            let lids: Vec<String> = self
                .db
                .borrow()
                .conn
                .prepare("SELECT logical_id FROM vault_dirty LIMIT ?1")?
                .query_map([BATCH as i64], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            if lids.is_empty() {
                return Ok(());
            }
            let mut records = Vec::with_capacity(lids.len());
            let mut notify_devices = false;
            // One more than every version this device has seen.
            let version: Version = (self.vault_clock()? + 1, s.account.device_id);
            let mut written = Vec::with_capacity(lids.len());
            for lid in &lids {
                if self.is_ephemeral_message(lid)? {
                    continue; // exists but must stay local only
                }
                let record = self.build_record(lid)?.unwrap_or(Record::Deleted { lid: lid.clone() });
                if !lid.starts_with("msg:") {
                    notify_devices = true;
                }
                let id = crypto::vault_record_id(&s.keys.vault_key, lid);
                // A record vouched for by the device that linked us and not received yet:
                // what we hold for it is a default, not the account's state. It is not
                // written over the real one (which replaces ours when it arrives).
                if self.vault_known(&id)?.is_some_and(|k| !k.applied) {
                    continue;
                }
                let sealed = seal_record(&s.keys.vault_key, &id, version, &serde_json::to_vec(&record)?)?;
                written.push((id.clone(), lid.clone()));
                records.push(VaultRecordDto { id, data: Some(b64(sealed)), seq: 0 });
            }
            if !records.is_empty() {
                let _: serde_json::Value = s.api.put("/v1/vault/records", &json!({ "records": records })).await?;
                for (id, lid) in &written {
                    self.vault_set_version(id, version, Some(lid), true)?;
                }
                self.vault_bump_clock(version.0)?;
            }
            {
                let db = self.db.borrow();
                for lid in &lids {
                    db.conn.execute("DELETE FROM vault_dirty WHERE logical_id = ?1", [lid])?;
                }
            }
            if notify_devices {
                self.enqueue(&super::send::Job::OwnDevices { content: Content::Sync(SyncMessage::VaultUpdated) })?;
            }
        }
    }

    fn vault_clock(&self) -> Result<u64> {
        Ok(self.db.borrow().kv_get_json("vault_clock")?.unwrap_or(0))
    }

    fn vault_bump_clock(&self, n: u64) -> Result<()> {
        if n > self.vault_clock()? {
            self.db.borrow().kv_set_json("vault_clock", &n)?;
        }
        Ok(())
    }

    fn vault_known(&self, record_id: &str) -> Result<Option<Known>> {
        Ok(self
            .db
            .borrow()
            .conn
            .query_row("SELECT n, d, lid, applied FROM vault_versions WHERE record_id = ?1", [record_id], |r| {
                Ok(Known { version: (r.get::<_, i64>(0)? as u64, r.get(1)?), lid: r.get(2)?, applied: r.get(3)? })
            })
            .optional()?)
    }

    /// Remembers the version of a record; never goes back to a lower one.
    fn vault_set_version(&self, record_id: &str, version: Version, lid: Option<&str>, applied: bool) -> Result<()> {
        if self.vault_known(record_id)?.is_some_and(|k| k.version > version || (k.version == version && k.applied && !applied)) {
            return Ok(());
        }
        self.db.borrow().conn.execute(
            "INSERT INTO vault_versions (record_id, n, d, lid, applied) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(record_id) DO UPDATE SET n = excluded.n, d = excluded.d, lid = COALESCE(excluded.lid, lid), applied = excluded.applied",
            params![record_id, version.0 as i64, version.1, lid, applied],
        )?;
        Ok(())
    }

    /// Takes one record from the server. Returns whether it was applied.
    async fn vault_take(&self, key: &[u8; 32], record_id: &str, data: &str) -> Result<bool> {
        let (version, plain) = open_record(key, record_id, &unb64(data)?)?;
        self.vault_bump_clock(version.0)?;
        let known = self.vault_known(record_id)?;
        // Vouched for by the device that linked us and received only now: whatever we hold
        // for it meanwhile is a default of this device, not a change of the user's.
        let vouched = known.as_ref().is_some_and(|k| !k.applied);
        if let Some(k) = known {
            if version < k.version {
                // An older copy than the one we have (a rollback, or two devices writing at
                // once): ours stays, and goes up again so that the vault has it.
                log::warn!("vault: record {record_id} came back older than known; keeping ours");
                if let (true, Some(lid)) = (k.applied, &k.lid) {
                    // Not in a loop: a server that keeps returning the old copy gets ours
                    // once per period.
                    let last = self.vault_healed.borrow().get(record_id).copied().unwrap_or(0);
                    if now_ms() - last > HEAL_EVERY_MS {
                        self.vault_healed.borrow_mut().insert(record_id.to_string(), now_ms());
                        self.mark_dirty(lid);
                    }
                }
                return Ok(false);
            }
            if version == k.version && k.applied {
                return Ok(false); // our own write, or already applied
            }
        }
        let rec: Record = serde_json::from_slice(&plain)?;
        let lid = lid_of(&rec);
        // Records from before versions do not authenticate their id: check it here, so one
        // record cannot be served in place of another.
        if crypto::vault_record_id(key, &lid) != record_id {
            return Err(crate::error::CoreError::internal("stored under another record's id"));
        }
        if let Err(e) = self.apply_record(rec, vouched).await {
            log::warn!("vault apply: {e}");
        }
        self.vault_set_version(record_id, version, Some(&lid), true)?;
        Ok(true)
    }

    /// Existing device, when it links another: writes the manifest of the vault as it knows it
    /// and returns what the new device must find. None: no anchor could be written (the new
    /// device then restores as after a sign-in with the password).
    pub(super) async fn vault_write_manifest(&self) -> Option<VaultFloor> {
        let write = async {
            self.vault_flush().await?; // everything pending first, so the manifest covers it
            let entries: Vec<ManifestEntry> = {
                let db = self.db.borrow();
                let mut st = db.conn.prepare(
                    "SELECT record_id, n, d FROM vault_versions
                     WHERE applied = 1 AND lid IS NOT NULL AND lid NOT LIKE 'msg:%' AND lid NOT LIKE 'manifest:%' ORDER BY record_id",
                )?;
                let rows = st.query_map([], |r| Ok((r.get(0)?, r.get::<_, i64>(1)? as u64, r.get(2)?)))?;
                rows.collect::<rusqlite::Result<_>>()?
            };
            let parts: Vec<Vec<ManifestEntry>> =
                if entries.is_empty() { vec![Vec::new()] } else { entries.chunks(MANIFEST_PART).map(<[_]>::to_vec).collect() };
            let mut content = Vec::new();
            for (i, entries) in parts.iter().enumerate() {
                content.extend(serde_json::to_vec(&Record::Manifest { part: i as u32, entries: entries.clone() })?);
            }
            self.db.borrow().kv_set_json("vault_manifest", &parts)?;
            for i in 0..parts.len() {
                self.mark_dirty(&format!("manifest:{i}"));
            }
            self.vault_flush().await?;
            Ok::<_, crate::error::CoreError>(VaultFloor { parts: parts.len() as u32, hash: b64(crypto::sha256(&content)) })
        };
        match write.await {
            Ok(floor) => Some(floor),
            Err(e) => {
                log::warn!("vault manifest: {e}");
                None
            }
        }
    }

    /// Newly linked device: loads the manifest the linking device vouched for and takes its
    /// versions as the minimum, before any record is applied. False: not there yet, wait.
    async fn vault_floor_ready(&self) -> Result<bool> {
        let s = self.session()?;
        let Some(floor) = self.db.borrow().kv_get_json::<VaultFloor>("vault_floor")? else { return Ok(true) };
        if self.db.borrow().kv_get_json::<bool>("vault_floor_seeded")?.unwrap_or(false) {
            return Ok(true);
        }
        let key = &s.keys.vault_key;
        let ids: Vec<String> = (0..floor.parts.min(1024)).map(|i| crypto::vault_record_id(key, &format!("manifest:{i}"))).collect();
        let mut found = std::collections::HashMap::new();
        let mut since = 0i64;
        for _ in 0..MAX_VAULT_PAGES_PER_PULL {
            let page: VaultPageDto = s.api.get(&format!("/v1/vault/records?since={since}&limit=500")).await?;
            for r in &page.records {
                since = since.max(r.seq);
                if let (true, Some(data)) = (ids.contains(&r.id), &r.data) {
                    found.insert(r.id.clone(), data.clone());
                }
            }
            if !page.more || page.records.is_empty() {
                break;
            }
        }
        let mut content = Vec::new();
        let mut entries: Vec<ManifestEntry> = Vec::new();
        let mut complete = true;
        for (i, id) in ids.iter().enumerate() {
            let part = found
                .get(id)
                .and_then(|d| unb64(d).ok())
                .and_then(|blob| open_record(key, id, &blob).ok())
                .and_then(|(_, plain)| match serde_json::from_slice::<Record>(&plain) {
                    Ok(Record::Manifest { part, entries }) if part as usize == i => Some((plain, entries)),
                    _ => None,
                });
            match part {
                Some((plain, e)) => {
                    content.extend(plain);
                    entries.extend(e);
                }
                None => {
                    complete = false;
                    break;
                }
            }
        }
        if complete && b64(crypto::sha256(&content)) == floor.hash {
            for (id, n, d) in &entries {
                if id.len() <= 128 {
                    self.vault_set_version(id, (*n, *d), None, false)?;
                    self.vault_bump_clock(*n)?;
                }
            }
            self.db.borrow().kv_set_json("vault_floor_seeded", &true)?;
            log::info!("vault: {} record versions vouched for by the linking device", entries.len());
            return Ok(true);
        }
        // The server does not have the manifest we were told about (yet, or any more).
        self.vault_request_resend()?;
        let since_login = now_ms() - self.db.borrow().kv_get_json::<i64>("vault_floor_at")?.unwrap_or(0);
        if since_login < FLOOR_WAIT_MS {
            return Ok(false);
        }
        log::warn!("vault: the manifest of the linking device never arrived; restoring without it");
        self.emit("vault_incomplete", &json!({ "reason": "manifest" }));
        let db = self.db.borrow();
        db.kv_delete("vault_floor")?;
        db.kv_set_json("restore_unanchored", &true)?;
        Ok(true)
    }

    /// After a complete pull on a newly linked device: is everything vouched for there?
    fn vault_floor_check(&self) -> Result<()> {
        let seeded = self.db.borrow().kv_get_json::<bool>("vault_floor_seeded")?.unwrap_or(false);
        if !seeded || self.db.borrow().kv_get("vault_floor")?.is_none() {
            return Ok(());
        }
        let missing: i64 = self.db.borrow().conn.query_row("SELECT COUNT(*) FROM vault_versions WHERE applied = 0", [], |r| r.get(0))?;
        if missing == 0 {
            self.db.borrow().kv_delete("vault_floor")?;
            log::info!("vault: complete, as vouched for by the linking device");
        } else {
            // Withheld or rolled back by the server: our other devices upload them again.
            log::warn!("vault: {missing} records are missing or older than vouched for");
            self.emit("vault_incomplete", &json!({ "missing": missing }));
            self.vault_request_resend()?;
        }
        Ok(())
    }

    /// Asks our other devices to upload their records again (at most once a minute).
    fn vault_request_resend(&self) -> Result<()> {
        let last: i64 = self.db.borrow().kv_get_json("vault_resend_asked")?.unwrap_or(0);
        if now_ms() - last < 60_000 {
            return Ok(());
        }
        self.db.borrow().kv_set_json("vault_resend_asked", &now_ms())?;
        self.enqueue(&super::send::Job::OwnDevices { content: Content::Sync(SyncMessage::VaultResend) })
    }

    /// A device of ours asked for the records again: everything except messages goes up under
    /// new versions, the last manifest included.
    pub(super) fn vault_resend_all(&self) -> Result<()> {
        let last: i64 = self.db.borrow().kv_get_json("vault_resent")?.unwrap_or(0);
        if now_ms() - last < 30_000 {
            return Ok(());
        }
        self.db.borrow().kv_set_json("vault_resent", &now_ms())?;
        let lids: Vec<String> = {
            let db = self.db.borrow();
            let mut st = db.conn.prepare("SELECT lid FROM vault_versions WHERE applied = 1 AND lid IS NOT NULL AND lid NOT LIKE 'msg:%'")?;
            let rows = st.query_map([], |r| r.get(0))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        for lid in &lids {
            self.mark_dirty(lid);
        }
        Ok(())
    }

    /// Once per database: records written before versions existed go up again with one.
    fn vault_upgrade_v2(&self) -> Result<()> {
        if self.db.borrow().kv_get_json::<bool>("vault_v2")?.unwrap_or(false) {
            return Ok(());
        }
        let ids = |sql: &str| -> Result<Vec<String>> {
            let db = self.db.borrow();
            let mut st = db.conn.prepare(sql)?;
            let rows = st.query_map([], |r| r.get(0))?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        };
        self.mark_dirty("settings");
        for id in ids("SELECT account_id FROM contacts")? {
            self.vault_put_contact(&id);
        }
        for id in ids("SELECT group_id FROM groups")? {
            self.vault_put_group(&id);
        }
        for id in ids("SELECT id FROM conversations")? {
            self.vault_put_conversation(&id);
        }
        if self.backup_enabled() && !self.temporary_mode() {
            self.vault_backfill_messages()?;
        }
        self.db.borrow().kv_set_json("vault_v2", &true)
    }

    /// Pulls what changed in the vault. False: a newly linked device still waits for the
    /// manifest (nothing was applied, and nothing must be written yet).
    async fn vault_pull(&self) -> Result<bool> {
        let s = self.session()?;
        if !self.vault_floor_ready().await? {
            return Ok(false);
        }
        let restoring: bool = self.db.borrow().kv_get_json("restore_pending")?.unwrap_or(false);
        let mut applied = 0usize;
        let mut complete = false;
        // A server answering `more: true` forever must not keep this loop (and the device)
        // busy: the rest is pulled by the next sync.
        for page_no in 0.. {
            if page_no >= MAX_VAULT_PAGES_PER_PULL {
                log::warn!("vault: stopping after {page_no} pages; continuing at the next sync");
                break;
            }
            let since: i64 = self.db.borrow().kv_get_json("vault_seq")?.unwrap_or(0);
            let page: VaultPageDto = s.api.get(&format!("/v1/vault/records?since={since}&limit=500")).await?;
            let mut max_seq = since;
            for r in &page.records {
                max_seq = max_seq.max(r.seq);
                let Some(data) = &r.data else { continue };
                match self.vault_take(&s.keys.vault_key, &r.id, data).await {
                    Ok(true) => applied += 1,
                    Ok(false) => {}
                    Err(e) => log::warn!("vault record {}: {e}", r.id),
                }
            }
            self.db.borrow().kv_set_json("vault_seq", &max_seq)?;
            if restoring {
                self.emit("history_sync", &json!({ "done": false, "records": applied }));
            }
            if !page.more {
                complete = true;
                break;
            }
        }
        if restoring {
            self.db.borrow().kv_set_json("restore_pending", &false)?;
            self.emit("history_sync", &json!({ "done": true, "records": applied }));
            // Pull groups and profiles now that keys are known.
            self.sync_all_groups().await;
            let me = self.me()?;
            let _ = self.fetch_profile(&me).await;
            self.refresh_stale_profiles().await;
        }
        if complete {
            self.vault_floor_check()?;
            // Restored with nothing but the server's vault: the profile key found there may
            // be one we had rotated away from. A new one replaces it.
            if self.db.borrow().kv_get_json::<bool>("restore_unanchored")?.unwrap_or(false) {
                self.db.borrow().kv_delete("restore_unanchored")?;
                self.request_profile_key_rotation()?;
            }
        }
        Ok(true)
    }

    async fn conv_for_target(&self, target: &Target) -> Result<String> {
        match target {
            Target::Peer { account_id } => self.ensure_direct_conversation(account_id, false).await,
            Target::Group { group_id } => self.create_group_conversation(group_id),
        }
    }

    /// `vouched`: see `vault_take` (the record wins over what this device holds).
    async fn apply_record(&self, rec: Record, vouched: bool) -> Result<()> {
        match rec {
            Record::Deleted { lid } => {
                if let Some(id) = lid.strip_prefix("msg:") {
                    // Disappearing messages expire on their own; ignore (old, buggy) tombstones.
                    if matches!(self.message_row(id)?, Some(m) if !m.is_disappearing()) {
                        // Deleted on another device.
                        let conv = self.message_row(id)?.map(|m| m.conversation_id);
                        self.drop_marks(id);
                        self.db.borrow().conn.execute("DELETE FROM messages WHERE id = ?1", [id])?;
                        if let Some(c) = conv {
                            self.emit("message_deleted", &json!({ "conversation_id": c, "message_id": id }));
                            self.emit_conversation(&c);
                        }
                    }
                }
            }
            Record::Settings { privacy, profile_key, dnd, calls, media_retention, default_timer } => {
                if let Some(t) = default_timer {
                    self.apply_synced_default_timer(t)?;
                }
                if let Some(r) = media_retention {
                    self.apply_synced_media_retention(r)?;
                }
                self.db.borrow().kv_set_json("privacy", &privacy)?;
                if let Some(d) = dnd {
                    self.apply_synced_dnd(d)?;
                }
                if let Some(c) = calls {
                    self.apply_synced_call_settings(c)?;
                }
                if let Some(k) = profile_key.as_deref().and_then(|k| unb64(k).ok()) {
                    self.set_rotated_profile_key(&k, false)?;
                }
            }
            Record::Contact { account_id, number, local_name, phone, blocked, verified, is_contact, profile_key, updated_at, identity_key } => {
                if let Some(key) = identity_key.as_deref().and_then(|k| unb64(k).ok()) {
                    self.pin_identity(&account_id, &key)?;
                }
                let local_updated: Option<i64> = self
                    .db
                    .borrow()
                    .conn
                    .query_row("SELECT updated_at FROM contacts WHERE account_id = ?1", [&account_id], |r| r.get(0))
                    .optional()?;
                if !vouched && local_updated.map(|u| u > updated_at).unwrap_or(false) {
                    return Ok(());
                }
                let pk = profile_key.and_then(|k| unb64(&k).ok());
                self.db.borrow().conn.execute(
                    "INSERT INTO contacts (account_id, number, local_name, phone, blocked, verified, is_contact, profile_key, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                     ON CONFLICT(account_id) DO UPDATE SET number = CASE WHEN excluded.number = '' THEN contacts.number ELSE excluded.number END,
                        local_name = excluded.local_name, phone = excluded.phone, blocked = excluded.blocked,
                        verified = excluded.verified, is_contact = excluded.is_contact,
                        profile_key = COALESCE(excluded.profile_key, contacts.profile_key), updated_at = excluded.updated_at",
                    params![account_id, number, local_name, phone, blocked, verified, is_contact, pk, updated_at],
                )?;
                if let Ok(c) = self.contact_model(&account_id) {
                    self.emit("contact_updated", &c);
                }
            }
            Record::Group { group_id, group_key, attested } => {
                // Our own devices' list wins over a TOFU one, never over a newer snapshot.
                if let Some(a) = attested {
                    let take = match self.attested(&group_id) {
                        None => true,
                        Some(cur) => cur.tofu || a.revision >= cur.revision,
                    };
                    if take {
                        self.set_attested(&group_id, &a)?;
                    }
                }
                if self.store_group_key(&group_id, &unb64(&group_key)?)? {
                    let _ = self.sync_group(&group_id, None).await;
                }
            }
            Record::Conversation { target, muted_until, archived, pinned, disappearing_seconds, accepted } => {
                let conv = self.conv_for_target(&target).await?;
                // Acceptance only ever goes one way (a stale record must not turn a chat back
                // into a request).
                self.db.borrow().conn.execute(
                    "UPDATE conversations SET muted_until = ?2, archived = ?3, pinned = ?4, disappearing_seconds = ?5,
                        accepted = MAX(accepted, ?6) WHERE id = ?1",
                    params![conv, muted_until, archived, pinned, disappearing_seconds, accepted],
                )?;
                self.emit_conversation(&conv);
            }
            Record::Manifest { .. } => {} // read by `vault_floor_ready`
            Record::Message(m) => {
                if !crate::util::is_id(&m.id) {
                    return Ok(());
                }
                let conv = self.conv_for_target(&m.target).await?;
                let exists = self.message_row(&m.id)?;
                if let Some(existing) = exists {
                    // A record only moves a message forward (newer edit, deletion): a stale copy
                    // uploaded before an edit must not bring the old text back.
                    let newer_edit = match (m.edited_at, existing.edited_at) {
                        (Some(a), Some(b)) => a > b,
                        (Some(_), None) => true,
                        _ => false,
                    };
                    if m.deleted && !existing.deleted {
                        self.db.borrow().conn.execute(
                            "UPDATE messages SET deleted = 1, body = NULL, edit_history_json = NULL WHERE id = ?1",
                            [&m.id],
                        )?;
                    } else if newer_edit && !existing.deleted {
                        let mentions_json = (!m.mentions.is_empty()).then(|| serde_json::to_string(&m.mentions)).transpose()?;
                        self.apply_edit(&m.id, m.body.as_deref().unwrap_or(""), mentions_json, m.edited_at.unwrap_or(m.sent_at))?;
                    }
                    let db = self.db.borrow();
                    db.conn.execute("UPDATE messages SET read_local = MAX(read_local, ?2) WHERE id = ?1", params![m.id, m.read])?;
                    let rank = |s: &str| ["failed", "pending", "sent", "delivered", "read"].iter().position(|x| *x == s).unwrap_or(0);
                    // Our other devices store their sync copy as "sent" even while this device
                    // still waits for the recipient to have a device (engine/send.rs).
                    let waiting = existing.status_reason.as_deref() == Some("no_devices") && existing.status == "pending";
                    if rank(&m.status) > rank(&existing.status) && !waiting {
                        db.conn.execute("UPDATE messages SET status = ?2 WHERE id = ?1", params![m.id, m.status])?;
                    }
                } else {
                    // Media past the retention limit is stored erased by `insert_message`.
                    let attachments = m.attachments.iter().filter_map(restored_attachment).collect();
                    // A call entry backed up before calls disappeared with their chat: in a chat
                    // with a timer it gets the deadline it would have had (and goes at once if
                    // that is past), instead of staying forever on the restored device.
                    let call_timer = m
                        .system
                        .as_ref()
                        .filter(|s| s.kind == "call")
                        .and_then(|_| self.conversation_row(&conv).ok())
                        .map(|c| c.disappearing_seconds)
                        .filter(|t| *t > 0)
                        .unwrap_or(0);
                    self.insert_message(NewMessage {
                        id: m.id.clone(),
                        conversation_id: conv.clone(),
                        sender_account_id: m.sender_account_id.clone(),
                        sender_device: 0,
                        outgoing: m.outgoing,
                        body: if m.deleted { None } else { m.body.clone() },
                        attachments,
                        location: m.location.clone(),
                        contacts: m.contacts.clone(),
                        mentions: m.mentions.clone(),
                        reply_to_id: m.reply_to_id.clone(),
                        forwarded: m.forwarded,
                        status: m.status.clone(),
                        sent_at: m.sent_at,
                        received_at: m.received_at,
                        expires_in: call_timer,
                        expires_at: (call_timer > 0).then(|| m.sent_at.saturating_add(call_timer.saturating_mul(1000))),
                        system: m.system.clone(),
                    })?;
                    let db = self.db.borrow();
                    db.conn.execute(
                        "UPDATE messages SET read_local = ?2, edited_at = ?3, deleted = ?4 WHERE id = ?1",
                        params![m.id, m.read || m.outgoing, m.edited_at, m.deleted],
                    )?;
                }
                {
                    let db = self.db.borrow();
                    db.conn.execute("DELETE FROM reactions WHERE message_id = ?1", [&m.id])?;
                    for (acct, emoji) in &m.reactions {
                        db.conn.execute(
                            "INSERT OR REPLACE INTO reactions (message_id, account_id, emoji, ts) VALUES (?1, ?2, ?3, ?4)",
                            params![m.id, acct, emoji, m.sent_at],
                        )?;
                    }
                }
                self.recount_unread(&conv)?;
                self.emit_message(&m.id, false);
                self.emit_conversation(&conv);
            }
        }
        Ok(())
    }

    pub(super) async fn vault_loop(self: Rc<Self>) {
        let mut pulled_once = false;
        // Complete pulls since this engine started.
        let mut pulls = 0u32;
        loop {
            if self.ws_out.borrow().is_some() {
                if let Err(e) = self.vault_upgrade_v2() {
                    log::info!("vault upgrade: {e}");
                }
                // Pull first: what we write next is versioned above everything seen so far.
                match self.vault_pull().await {
                    Ok(ready) => {
                        pulled_once = ready;
                        if ready {
                            pulls = pulls.saturating_add(1);
                            if let Err(e) = self.vault_flush().await {
                                log::info!("vault flush: {e}");
                            }
                        }
                    }
                    Err(e) => log::info!("vault pull: {e}"),
                }
            }
            let notify = self.vault_notify.clone();
            // A newly linked device still short of what was vouched for looks again soon.
            let incomplete = matches!(self.db.borrow().kv_get("vault_floor"), Ok(Some(_)));
            // So does a device that just started: another device of ours may be uploading
            // records at this very moment, and it tells only the devices it knows about (a
            // device that signed in seconds ago is not one of them yet).
            let wait = if pulled_once && !incomplete && pulls > SETTLING_PULLS { 300 } else { 10 };
            tokio::select! {
                _ = notify.notified() => {
                    // Coalesce bursts of writes.
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
                _ = tokio::time::sleep(Duration::from_secs(wait)) => {}
            }
            let _ = now_ms();
        }
    }
}

/// An attachment as backed up / restored: uploaded ones as a pointer to download again, media
/// erased by the retention (engine/retention.rs) as its stub (no key, nothing to download).
fn restored_attachment(a: &StoredAttachment) -> Option<StoredAttachment> {
    if super::retention::is_expired(a) {
        Some(super::retention::expired_attachment(a, a.expired_at.unwrap_or_else(now_ms)))
    } else if a.cdn_id.is_some() {
        Some(StoredAttachment { local_path: None, download_state: "pending".into(), ..a.clone() })
    } else {
        None
    }
}

fn vault_message(m: MessageRow, target: Target, reactions: Vec<(String, String)>, read: bool) -> VaultMessage {
    VaultMessage {
        id: m.id,
        target,
        sender_account_id: m.sender_account_id,
        outgoing: m.outgoing,
        body: m.body,
        attachments: m.attachments.iter().filter_map(restored_attachment).collect(),
        location: m.location,
        contacts: m.contacts,
        mentions: m.mentions,
        reply_to_id: m.reply_to_id,
        forwarded: m.forwarded,
        status: m.status,
        sent_at: m.sent_at,
        received_at: m.received_at,
        edited_at: m.edited_at,
        deleted: m.deleted,
        system: m.system,
        reactions,
        read,
    }
}

impl Inner {
    /// Pins an identity learned by another of our devices (or checked against the key
    /// transparency log for the safety number); never replaces a pinned one.
    pub(super) fn pin_identity(&self, account_id: &str, key: &[u8]) -> Result<()> {
        let db = self.db.borrow();
        let existing: Option<Vec<u8>> = db
            .conn
            .query_row("SELECT identity_key FROM identities WHERE account_id = ?1", [account_id], |r| r.get(0))
            .optional()?;
        match existing {
            None => {
                db.conn.execute(
                    "INSERT INTO identities (account_id, identity_key, first_seen) VALUES (?1, ?2, ?3)",
                    params![account_id, key, crate::util::now_ms()],
                )?;
            }
            Some(old) if old != key => {
                log::error!("vault identity for {account_id} differs from the pinned one; keeping the pinned key");
            }
            Some(_) => {}
        }
        Ok(())
    }
}

fn accepted_default() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_are_bound_to_their_id_and_version() {
        let key = [3u8; 32];
        let blob = seal_record(&key, "id-a", (7, 2), b"content").unwrap();
        assert_eq!(open_record(&key, "id-a", &blob).unwrap(), ((7, 2), b"content".to_vec()));
        // Served under another record's id.
        assert!(open_record(&key, "id-b", &blob).is_err());
        // A changed version (header) or content.
        for i in [1, 8, 9, 12, HEADER_LEN + 13] {
            let mut bad = blob.clone();
            bad[i] ^= 1;
            assert!(open_record(&key, "id-a", &bad).is_err(), "byte {i}");
        }
        // The header removed (to pass it off as a record from before versions).
        assert!(open_record(&key, "id-a", &blob[HEADER_LEN..]).is_err());
        assert!(open_record(&[4u8; 32], "id-a", &blob).is_err());
    }

    #[test]
    fn records_from_before_versions_count_as_the_lowest_version() {
        let key = [3u8; 32];
        let legacy = crypto::aead_seal(&key, b"old", VAULT_AAD).unwrap();
        assert_eq!(open_record(&key, "any", &legacy).unwrap(), ((0, 0), b"old".to_vec()));
        // Also when its random nonce happens to start like a versioned record.
        let mut tries = 0;
        loop {
            let l = crypto::aead_seal(&key, b"old", VAULT_AAD).unwrap();
            tries += 1;
            if l[0] == BLOB_V2 || tries > 20_000 {
                assert_eq!(open_record(&key, "any", &l).unwrap().0, (0, 0));
                break;
            }
        }
    }

    #[test]
    fn versions_order_by_counter_then_device() {
        let (a, b, c): (Version, Version, Version) = ((5, 9), (6, 1), (6, 2));
        assert!(a < b && b < c && (0, 0) < a);
    }

    #[test]
    fn every_record_names_its_logical_id() {
        let peer = Record::Conversation {
            target: Target::Peer { account_id: "p".into() },
            muted_until: None,
            archived: false,
            pinned: false,
            disappearing_seconds: 0,
            accepted: true,
        };
        assert_eq!(lid_of(&peer), "conv:peer:p");
        assert_eq!(lid_of(&Record::Deleted { lid: "msg:x".into() }), "msg:x");
        assert_eq!(lid_of(&Record::Manifest { part: 3, entries: vec![] }), "manifest:3");
    }
}
