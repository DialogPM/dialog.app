//! Private groups v2 (zkgroup, docs/ZKGROUP-DESIGN.md stage 3).
//!
//! A v2 group is identified locally like a v1 group (a UUID string), but that id is derived from
//! a random 32-byte master key; the server identifies the group by the GroupIdentifier of the
//! master key's public parameters instead, and cannot link the two. The master key travels like
//! the v1 group key: in pairwise group control messages (and the vault), so the attestation layer
//! (engine/attest.rs) keeps working unchanged on top.
//!
//! The server keeps only UuidCiphertexts for members and blobs encrypted with the group secret
//! params for the title/description/avatar ("attributes") and the rules ("settings"). Every
//! request is anonymous and carries a zkgroup auth credential presentation (engine/zk.rs).
//! The decrypted state is stored in the same local `groups` row as a v1 group (see
//! `apply_group_state`), so the rest of the engine and the apps see no difference.
//!
//! Sending still uses sender keys; the client names the recipient accounts (decrypted members,
//! intersected with the attested list) and the server fans out (stage 4 will remove the sender
//! and recipient list from the server's view with group send endorsements).

use hkdf::Hkdf;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::Sha256;
use zkgroup::groups::{GroupMasterKey, GroupSecretParams, UuidCiphertext};

use super::Inner;
use crate::api::{ApiErrorOrNet, GroupDto, GroupMemberDto, GroupSendReq, GroupSettingsDto, RecipientDto, ServerInfoDto};
use crate::content::{GroupChange, GroupMetadata};
use crate::crypto::{b64, unb64};
use crate::error::{CoreError, ErrorCode, Result};
use crate::model::{GroupInfo, GroupSettingsPatch};
use crate::util::now_ms;

const LOCAL_ID_INFO: &[u8] = b"chatapp/group-v2/local-id";
const SUPPORTED_KEY: &str = "groups_v2_supported";
const DEFAULT_KEY: &str = "groups_v2_default";
/// Blobs are padded to a multiple of this, so their size says little about the title.
const PAD_TO: usize = 128;
const ATTRIBUTES_TAG: u8 = b'A';
const SETTINGS_TAG: u8 = b'S';
/// Must match the server (server-rs/src/zk.rs).
const CHANGE_CONTEXT: &[u8] = b"chatapp/group-v2/change\0";

/// The local id of the v2 group with this master key (a v4-shaped UUID string).
pub fn local_group_id(master_key: &[u8; 32]) -> String {
    let hk = Hkdf::<Sha256>::new(None, master_key);
    let mut out = [0u8; 16];
    hk.expand(LOCAL_ID_INFO, &mut out).expect("16 bytes");
    uuid::Builder::from_random_bytes(out).into_uuid().to_string()
}

pub fn secret_params(master_key: &[u8; 32]) -> GroupSecretParams {
    GroupSecretParams::derive_from_master_key(GroupMasterKey::new(*master_key))
}

fn randomness() -> [u8; zkgroup::RANDOMNESS_LEN] {
    crate::crypto::random_bytes::<{ zkgroup::RANDOMNESS_LEN }>()
}

/// The ciphertext of an account in the group (deterministic: the server compares them).
pub fn encrypt_member(params: &GroupSecretParams, account_id: &str) -> Result<Vec<u8>> {
    let uuid = uuid::Uuid::parse_str(account_id).map_err(|_| CoreError::invalid("invalid account id"))?;
    Ok(zkgroup::serialize(&params.encrypt_service_id(libsignal_core::Aci::from(uuid).into())))
}

/// The account id in a member ciphertext (None: not a valid ciphertext of this group).
pub fn decrypt_member(params: &GroupSecretParams, uid: &[u8]) -> Option<String> {
    let c: UuidCiphertext = zkgroup::deserialize(uid).ok()?;
    match params.decrypt_service_id(c).ok()? {
        libsignal_core::ServiceId::Aci(aci) => Some(uuid::Uuid::from(aci).to_string()),
        _ => None,
    }
}

fn seal_blob(params: &GroupSecretParams, tag: u8, value: &impl Serialize) -> Result<String> {
    let mut plain = vec![tag];
    plain.extend(serde_json::to_vec(value)?);
    let padding = (PAD_TO - plain.len() % PAD_TO) % PAD_TO;
    Ok(b64(params.encrypt_blob_with_padding(randomness(), &plain, padding as u32)))
}

fn open_blob<T: for<'de> Deserialize<'de>>(params: &GroupSecretParams, tag: u8, blob: &str) -> Option<T> {
    let plain = params.decrypt_blob_with_padding(&unb64(blob).ok()?).ok()?;
    let (t, json) = plain.split_first()?;
    if *t != tag {
        return None;
    }
    serde_json::from_slice(json).ok()
}

/// The group rules, encrypted ("settings" blob; only admins change it). Who may edit the
/// attributes is the server-enforced access level instead.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SettingsBlob {
    #[serde(default)]
    pub only_admins_send: bool,
    #[serde(default)]
    pub disappearing_seconds: i64,
    #[serde(default)]
    pub hide_members: bool,
    #[serde(default)]
    pub member_names: String,
    #[serde(default)]
    pub created_by: Option<String>,
    #[serde(default)]
    pub created_at: i64,
}

impl SettingsBlob {
    fn apply(&mut self, p: &GroupSettingsPatch) {
        if let Some(v) = p.only_admins_send {
            self.only_admins_send = v;
        }
        if let Some(v) = p.disappearing_seconds {
            self.disappearing_seconds = v;
        }
        if let Some(v) = p.hide_members {
            self.hide_members = v;
        }
        if let Some(v) = &p.member_names {
            self.member_names = v.clone();
        }
    }
}

#[derive(Debug, Deserialize)]
struct AccessDto {
    attributes: String,
    members: String,
}

#[derive(Debug, Deserialize)]
struct MemberDto {
    uid: String,
    role: String,
    joined_revision: i64,
}

#[derive(Debug, Deserialize)]
struct PendingDto {
    uid: String,
}

#[derive(Debug, Deserialize)]
struct StateDto {
    revision: i64,
    attributes: String,
    settings: String,
    access: AccessDto,
    members: Vec<MemberDto>,
    #[serde(default)]
    pending: Vec<PendingDto>,
}

/// A decrypted server state.
#[derive(Debug, Clone)]
pub struct DecryptedState {
    pub revision: i64,
    pub attributes: Option<GroupMetadata>,
    pub settings: SettingsBlob,
    pub only_admins_edit: bool,
    pub only_admins_add: bool,
    /// (account id, role, joined revision)
    pub members: Vec<(String, String, i64)>,
    pub pending: Vec<String>,
}

fn decrypt_state(params: &GroupSecretParams, st: StateDto) -> DecryptedState {
    let members = st
        .members
        .iter()
        .filter_map(|m| {
            let account = unb64(&m.uid).ok().and_then(|u| decrypt_member(params, &u));
            if account.is_none() {
                log::warn!("group v2: undecryptable member entry");
            }
            Some((account?, if m.role == "admin" { "admin".to_string() } else { "member".to_string() }, m.joined_revision))
        })
        .collect();
    DecryptedState {
        revision: st.revision,
        attributes: open_blob(params, ATTRIBUTES_TAG, &st.attributes),
        settings: open_blob(params, SETTINGS_TAG, &st.settings).unwrap_or_default(),
        only_admins_edit: st.access.attributes == "admin",
        only_admins_add: st.access.members == "admin",
        members,
        pending: st.pending.iter().filter_map(|p| unb64(&p.uid).ok().and_then(|u| decrypt_member(params, &u))).collect(),
    }
}

fn role(admin: bool) -> &'static str {
    if admin {
        "admin"
    } else {
        "member"
    }
}

impl Inner {
    /// Remembers the server's v2 capability (from /v1/info, at each connection start).
    pub(super) fn note_groups_v2(&self, info: &ServerInfoDto) -> Result<()> {
        let db = self.db.borrow();
        db.kv_set_json(SUPPORTED_KEY, &info.groups_v2)?;
        db.kv_set_json(DEFAULT_KEY, &info.groups_v2_default)
    }

    /// Whether a new group is created as v2: the server must support it (and have pinned zkgroup
    /// parameters); `requested` overrides the server's default (Some(1) / Some(2)).
    pub(super) fn create_as_v2(&self, requested: Option<u8>) -> Result<bool> {
        let db = self.db.borrow();
        let supported = db.kv_get_json::<bool>(SUPPORTED_KEY)?.unwrap_or(false) && db.kv_get_json::<String>("zk_public_params")?.is_some();
        match requested {
            Some(2) if !supported => Err(CoreError::new(ErrorCode::Forbidden, "this server has no private groups")),
            Some(2) => Ok(true),
            Some(_) => Ok(false),
            None => Ok(supported && db.kv_get_json::<bool>(DEFAULT_KEY)?.unwrap_or(false)),
        }
    }

    /// Whether the group is a v2 group (its key is a master key that derives its id).
    pub fn is_group_v2(&self, group_id: &str) -> bool {
        self.group_key(group_id).is_ok_and(|k| local_group_id(&k) == group_id)
    }

    /// A key received for a v2 group must be its master key (anything else would detach the
    /// local id from the server's group).
    pub(super) fn acceptable_group_key(&self, group_id: &str, key: &[u8]) -> bool {
        if !self.is_group_v2(group_id) {
            return true;
        }
        <[u8; 32]>::try_from(key).is_ok_and(|k| local_group_id(&k) == group_id)
    }

    fn v2_params(&self, group_id: &str) -> Result<GroupSecretParams> {
        let key = self.group_key(group_id)?;
        if local_group_id(&key) != group_id {
            return Err(CoreError::internal("not a v2 group"));
        }
        Ok(secret_params(&key))
    }

    /// One anonymous request to the v2 API, authorized by a fresh presentation.
    async fn v2_request<T: serde::de::DeserializeOwned>(
        &self,
        params: &GroupSecretParams,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
    ) -> std::result::Result<T, ApiErrorOrNet> {
        let pres = self.zk_auth_presentation(params).await.map_err(ApiErrorOrNet::Net)?;
        let auth = format!("{}:{}", b64(zkgroup::serialize(&params.get_public_params())), b64(pres));
        let api = self.session().map_err(ApiErrorOrNet::Net)?.api.clone();
        api.group_v2_json(method, path, &auth, body).await
    }

    /// Like `v2_request`, with errors as CoreErrors (a rejected credential is not a session
    /// error: it must not log us out).
    async fn v2_call<T: serde::de::DeserializeOwned>(&self, params: &GroupSecretParams, method: reqwest::Method, path: &str, body: Option<&Value>) -> Result<T> {
        match self.v2_request(params, method, path, body).await {
            Ok(v) => Ok(v),
            Err(ApiErrorOrNet::Api(e)) if e.status.as_u16() == 401 => Err(CoreError::internal(format!("group credential rejected: {}", e.message))),
            Err(e) => Err(e.into()),
        }
    }

    async fn v2_state(&self, params: &GroupSecretParams) -> Result<DecryptedState> {
        let st: StateDto = self.v2_call(params, reqwest::Method::GET, "/v2/groups", None).await?;
        Ok(decrypt_state(params, st))
    }

    /// Creates the group on the server; returns its local id and master key.
    pub(super) async fn create_group_v2_on_server(
        &self,
        md: &GroupMetadata,
        settings: &GroupSettingsPatch,
        members: &[String],
    ) -> Result<(String, [u8; 32])> {
        let key = crate::crypto::random_bytes::<32>();
        let params = secret_params(&key);
        let me = self.me()?;
        let mut blob = SettingsBlob { member_names: "number".into(), created_by: Some(me.clone()), created_at: now_ms(), ..Default::default() };
        blob.apply(settings);
        let mut list = vec![json!({ "uid": b64(encrypt_member(&params, &me)?), "role": "admin" })];
        for m in members.iter().filter(|m| **m != me) {
            list.push(json!({ "uid": b64(encrypt_member(&params, m)?), "role": "member" }));
        }
        let body = json!({
            "attributes": seal_blob(&params, ATTRIBUTES_TAG, md)?,
            "settings": seal_blob(&params, SETTINGS_TAG, &blob)?,
            "attributes_access": role(settings.only_admins_edit.unwrap_or(false)),
            "members_access": "admin",
            "members": list,
        });
        let r: Value = self.v2_call(&params, reqwest::Method::POST, "/v2/groups", Some(&body)).await?;
        self.verify_v2_change(&params, &r);
        Ok((local_group_id(&key), key))
    }

    /// Checks the server's signature on a change it returned (logged: the change is applied).
    fn verify_v2_change(&self, params: &GroupSecretParams, r: &Value) -> bool {
        let (Some(change), Some(sig)) = (r["change"].as_str(), r["signature"].as_str()) else { return false };
        let ok = (|| {
            let server = self.zk_public_params().ok()?;
            let change = unb64(change).ok()?;
            let sig: [u8; 64] = unb64(sig).ok()?.try_into().ok()?;
            let mut msg = CHANGE_CONTEXT.to_vec();
            msg.extend_from_slice(&params.get_group_identifier());
            msg.extend_from_slice(&change);
            server.verify_signature(&msg, sig).ok()
        })()
        .is_some();
        if !ok {
            log::error!("group v2: the server's signature on a change does not verify");
        }
        ok
    }

    /// Applies actions as us, retrying when the group moved on in between.
    async fn v2_modify(&self, group_id: &str, actions: Vec<Value>) -> Result<()> {
        let params = self.v2_params(group_id)?;
        let mut revision: i64 = self
            .db
            .borrow()
            .conn
            .query_row("SELECT revision FROM groups WHERE group_id = ?1", [group_id], |r| r.get(0))?;
        for _ in 0..3 {
            let body = json!({ "revision": revision + 1, "actions": actions });
            match self.v2_request::<Value>(&params, reqwest::Method::PATCH, "/v2/groups", Some(&body)).await {
                Ok(r) => {
                    self.verify_v2_change(&params, &r);
                    return Ok(());
                }
                Err(ApiErrorOrNet::Api(e)) if e.status.as_u16() == 409 => {
                    revision = e.body["revision"].as_i64().ok_or_else(|| CoreError::internal("group revision conflict"))?;
                }
                Err(ApiErrorOrNet::Api(e)) if e.status.as_u16() == 401 => {
                    return Err(CoreError::internal(format!("group credential rejected: {}", e.message)))
                }
                Err(e) => return Err(e.into()),
            }
        }
        Err(CoreError::internal("the group kept changing"))
    }

    /// Fetches and decrypts the server state into the local group row.
    pub(super) async fn sync_group_v2(&self, group_id: &str) -> Result<()> {
        let params = self.v2_params(group_id)?;
        let me = self.me()?;
        let mut state = match self.v2_state(&params).await {
            Ok(s) => s,
            Err(e) if e.code == ErrorCode::Forbidden || e.code == ErrorCode::NotFound => {
                self.mark_group_left(group_id)?;
                return Ok(());
            }
            Err(e) => return Err(e),
        };
        // Invited (pending): accept; whether the conversation shows as an invitation is decided
        // locally (engine/requests.rs), as for v1 groups.
        if state.pending.contains(&me) && !state.members.iter().any(|m| m.0 == me) {
            self.v2_modify_from(group_id, state.revision, vec![json!({ "type": "accept_pending" })]).await?;
            state = self.v2_state(&params).await?;
        }
        let dto = GroupDto {
            group_id: group_id.to_string(),
            revision: state.revision,
            metadata: String::new(),
            avatar_id: state.attributes.as_ref().and_then(|a| a.avatar.as_ref().map(|a| a.cdn_id.clone())),
            created_by: state.settings.created_by.clone(),
            created_at: state.settings.created_at,
            settings: GroupSettingsDto {
                only_admins_send: state.settings.only_admins_send,
                only_admins_edit: state.only_admins_edit,
                disappearing_seconds: state.settings.disappearing_seconds,
                hide_members: state.settings.hide_members,
                member_names: Some(if state.settings.member_names.is_empty() { "number".into() } else { state.settings.member_names.clone() }),
            },
            members: state
                .members
                .iter()
                .map(|(a, r, j)| GroupMemberDto { account_id: a.clone(), role: r.clone(), joined_at: *j, added_by: None })
                .collect(),
            invite: None,
            join_requests: 0,
        };
        let mut md = state.attributes;
        if let Some(m) = md.as_mut() {
            m.invite = None; // invite links are v1-only until stage 5
        }
        self.apply_group_state(group_id, &dto, md).await
    }

    async fn v2_modify_from(&self, group_id: &str, revision: i64, actions: Vec<Value>) -> Result<()> {
        self.db
            .borrow()
            .conn
            .execute("UPDATE groups SET revision = ?2 WHERE group_id = ?1", rusqlite::params![group_id, revision])?;
        self.v2_modify(group_id, actions).await
    }

    fn mark_group_left(&self, group_id: &str) -> Result<()> {
        let left: Option<bool> = {
            use rusqlite::OptionalExtension;
            self.db
                .borrow()
                .conn
                .query_row("SELECT left_group FROM groups WHERE group_id = ?1", [group_id], |r| r.get(0))
                .optional()?
        };
        if left == Some(false) {
            self.db.borrow().conn.execute("UPDATE groups SET left_group = 1 WHERE group_id = ?1", [group_id])?;
            self.group_devices.borrow_mut().remove(group_id);
            self.emit_group(group_id);
        }
        Ok(())
    }

    /// The attributes currently on the server (to change one field without losing another).
    pub(super) async fn current_attributes_v2(&self, group_id: &str) -> Option<GroupMetadata> {
        let params = self.v2_params(group_id).ok()?;
        self.v2_state(&params).await.ok()?.attributes
    }

    pub(super) async fn set_attributes_v2(&self, group_id: &str, md: &GroupMetadata) -> Result<()> {
        let params = self.v2_params(group_id)?;
        let md = GroupMetadata { invite: None, ..md.clone() };
        self.v2_modify(group_id, vec![json!({ "type": "modify_attributes", "blob": seal_blob(&params, ATTRIBUTES_TAG, &md)? })]).await
    }

    pub(super) async fn set_settings_v2(&self, group_id: &str, patch: &GroupSettingsPatch) -> Result<()> {
        let params = self.v2_params(group_id)?;
        let state = self.v2_state(&params).await?;
        let mut blob = state.settings.clone();
        blob.apply(patch);
        let mut actions = Vec::new();
        if blob != state.settings {
            actions.push(json!({ "type": "modify_settings", "blob": seal_blob(&params, SETTINGS_TAG, &blob)? }));
        }
        if let Some(v) = patch.only_admins_edit.filter(|v| *v != state.only_admins_edit) {
            actions.push(json!({ "type": "modify_access", "attributes": role(v), "members": role(state.only_admins_add) }));
        }
        if actions.is_empty() {
            return Ok(());
        }
        self.v2_modify_from(group_id, state.revision, actions).await
    }

    pub(super) async fn add_members_v2(&self, group_id: &str, accounts: &[String]) -> Result<()> {
        let params = self.v2_params(group_id)?;
        let actions = accounts
            .iter()
            .map(|a| Ok(json!({ "type": "add_member", "uid": b64(encrypt_member(&params, a)?), "role": "member" })))
            .collect::<Result<Vec<_>>>()?;
        self.v2_modify(group_id, actions).await
    }

    pub(super) async fn remove_member_v2(&self, group_id: &str, account: &str) -> Result<()> {
        let params = self.v2_params(group_id)?;
        self.v2_modify(group_id, vec![json!({ "type": "remove_member", "uid": b64(encrypt_member(&params, account)?) })]).await
    }

    pub(super) async fn set_role_v2(&self, group_id: &str, account: &str, admin: bool) -> Result<()> {
        let params = self.v2_params(group_id)?;
        self.v2_modify(group_id, vec![json!({ "type": "modify_role", "uid": b64(encrypt_member(&params, account)?), "role": role(admin) })]).await
    }

    /// Removes a member (we are an admin). Unlike v1 the key does not change: the master key
    /// identifies the group, and the server refuses the removed member's presentations.
    pub(super) async fn remove_group_member_v2(&self, conv_id: &str, gid: &str, account_id: &str) -> Result<GroupInfo> {
        let me = self.me()?;
        self.remove_member_v2(gid, account_id).await?;
        // Syncing rotates our sender key (a member is gone).
        self.sync_group(gid, None).await?;
        let rev: i64 = self.db.borrow().conn.query_row("SELECT revision FROM groups WHERE group_id = ?1", [gid], |r| r.get(0))?;
        self.attest_local(gid, |a| {
            a.tofu = false;
            a.revision = rev;
            a.members.remove(account_id);
        })?;
        // The removed member is told too (it learns it is out when its sync is refused).
        let id = self.broadcast_group_control(gid, GroupChange::MemberRemoved { account: account_id.to_string() }, &[account_id.to_string()])?;
        self.insert_system_message(conv_id, "group_member_removed", Some(me), vec![account_id.to_string()], None, Some(format!("grp-{id}")), now_ms())?;
        self.group_info(conv_id)
    }

    /// Recipients of a group send: devices of the decrypted members that are also attested
    /// (the server cannot add anyone to our sends).
    pub(super) async fn group_recipients_v2(&self, group_id: &str) -> Result<Vec<RecipientDto>> {
        let accounts: Vec<String> = self.group_member_ids(group_id)?.into_iter().filter(|a| self.is_attested_member(group_id, a)).collect();
        #[derive(Deserialize)]
        struct Resp {
            devices: Vec<RecipientDto>,
        }
        let s = self.session()?;
        let r: Resp = s.api.post("/v2/groups/devices", &json!({ "accounts": accounts })).await?;
        Ok(r
            .devices
            .into_iter()
            .filter(|d| !(d.account_id == s.account.account_id && d.device_id == s.account.device_id))
            .collect())
    }

    /// Sends a sender key ciphertext to the given devices, v1 or v2 route.
    pub(super) async fn put_group_messages_any(&self, group_id: &str, req: &GroupSendReq) -> std::result::Result<Value, ApiErrorOrNet> {
        let s = self.session().map_err(ApiErrorOrNet::Net)?;
        if self.is_group_v2(group_id) {
            let body = json!({
                "group_id": group_id,
                "timestamp": req.timestamp,
                "online": req.online,
                "content": req.content,
                "recipients": req.recipients,
            });
            s.api.send_json(reqwest::Method::PUT, "/v2/groups/messages", Some(&body)).await
        } else {
            s.api.put_group_messages(group_id, req).await
        }
    }

    /// The GroupIdentifier the server knows a v2 group by, in hex (tests, diagnostics).
    pub fn group_v2_server_id(&self, group_id: &str) -> Result<String> {
        Ok(self.v2_params(group_id)?.get_group_identifier().iter().map(|b| format!("{b:02X}")).collect())
    }

    /// The signed change log since `since` (each signature checked).
    pub async fn group_v2_changes(&self, group_id: &str, since: i64) -> Result<Vec<Value>> {
        let params = self.v2_params(group_id)?;
        let r: Value = self.v2_call(&params, reqwest::Method::GET, &format!("/v2/groups/changes?since={since}"), None).await?;
        let mut out = Vec::new();
        for c in r["changes"].as_array().cloned().unwrap_or_default() {
            if !self.verify_v2_change(&params, &c) {
                return Err(CoreError::internal("invalid signature on a group change"));
            }
            let bytes = unb64(c["change"].as_str().unwrap_or_default())?;
            out.push(serde_json::from_slice(&bytes)?);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn member_entries_roundtrip() {
        let key = crate::crypto::random_bytes::<32>();
        let params = secret_params(&key);
        let account = uuid::Uuid::new_v4().to_string();
        let c = encrypt_member(&params, &account).unwrap();
        assert_eq!(c.len(), 65);
        // Deterministic (the server matches presentations against it) and reversible.
        assert_eq!(encrypt_member(&params, &account).unwrap(), c);
        assert_eq!(decrypt_member(&params, &c).as_deref(), Some(account.as_str()));
        // The ciphertext does not contain the account id.
        let raw = uuid::Uuid::parse_str(&account).unwrap();
        assert!(!c.windows(16).any(|w| w == raw.as_bytes()));
        // Another group's key neither decrypts it nor produces the same ciphertext.
        let other = secret_params(&crate::crypto::random_bytes::<32>());
        assert_eq!(decrypt_member(&other, &c), None);
        assert_ne!(encrypt_member(&other, &account).unwrap(), c);
        assert_eq!(decrypt_member(&params, &[0xffu8; 65]), None);
        assert!(encrypt_member(&params, "not-a-uuid").is_err());
    }

    #[test]
    fn blobs_roundtrip_and_are_typed() {
        let params = secret_params(&crate::crypto::random_bytes::<32>());
        let md = GroupMetadata { name: "Team".into(), description: "d".into(), avatar: None, invite: None };
        let sealed = seal_blob(&params, ATTRIBUTES_TAG, &md).unwrap();
        let raw = unb64(&sealed).unwrap();
        assert!(!raw.windows(4).any(|w| w == b"Team"));
        let back: GroupMetadata = open_blob(&params, ATTRIBUTES_TAG, &sealed).unwrap();
        assert_eq!(back.name, "Team");
        // The server cannot pass the attributes off as settings, nor another group's blob.
        assert!(open_blob::<SettingsBlob>(&params, SETTINGS_TAG, &sealed).is_none());
        let other = secret_params(&crate::crypto::random_bytes::<32>());
        assert!(open_blob::<GroupMetadata>(&other, ATTRIBUTES_TAG, &sealed).is_none());
        // Padding hides the exact length.
        let longer = seal_blob(&params, ATTRIBUTES_TAG, &GroupMetadata { name: "Team!".into(), ..md }).unwrap();
        assert_eq!(unb64(&longer).unwrap().len(), raw.len());
    }

    #[test]
    fn local_id_is_derived_and_unlinkable() {
        let key = crate::crypto::random_bytes::<32>();
        let id = local_group_id(&key);
        assert_eq!(id, local_group_id(&key));
        assert!(uuid::Uuid::parse_str(&id).is_ok());
        assert_ne!(id, local_group_id(&crate::crypto::random_bytes::<32>()));
        // Unrelated to the identifier the server sees.
        let ident = secret_params(&key).get_group_identifier();
        let uuid = uuid::Uuid::parse_str(&id).unwrap();
        assert!(!ident.windows(8).any(|w| w == &uuid.as_bytes()[..8]));
    }

    #[test]
    fn decrypts_server_state() {
        let key = crate::crypto::random_bytes::<32>();
        let params = secret_params(&key);
        let (a, b) = (uuid::Uuid::new_v4().to_string(), uuid::Uuid::new_v4().to_string());
        let st: StateDto = serde_json::from_value(json!({
            "revision": 3,
            "attributes": seal_blob(&params, ATTRIBUTES_TAG, &GroupMetadata { name: "G".into(), ..Default::default() }).unwrap(),
            "settings": seal_blob(&params, SETTINGS_TAG, &SettingsBlob { disappearing_seconds: 60, ..Default::default() }).unwrap(),
            "access": { "attributes": "admin", "members": "admin" },
            "members": [
                { "uid": b64(encrypt_member(&params, &a).unwrap()), "role": "admin", "joined_revision": 1 },
                { "uid": b64([0xffu8; 65]), "role": "member", "joined_revision": 2 },
            ],
            "pending": [{ "uid": b64(encrypt_member(&params, &b).unwrap()), "role": "member", "added_by": "", "added_revision": 3 }],
        }))
        .unwrap();
        let d = decrypt_state(&params, st);
        assert_eq!(d.revision, 3);
        assert_eq!(d.members, vec![(a, "admin".to_string(), 1)]);
        assert_eq!(d.pending, vec![b]);
        assert_eq!(d.attributes.unwrap().name, "G");
        assert_eq!(d.settings.disappearing_seconds, 60);
        assert!(d.only_admins_edit);
    }
}
