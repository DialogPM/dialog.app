//! Groups: membership is managed by the server, metadata is end-to-end encrypted with a
//! group key distributed pairwise, and messages use Signal Sender Keys (see send.rs).

use std::collections::HashSet;

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::send::Job;
use super::Inner;
use crate::api::{GroupDto, GroupRefDto};
use crate::content::{AvatarRef, Content, GroupChange, GroupControl, GroupMetadata};
use crate::crypto::{self, b64, unb64};
use crate::error::{CoreError, ErrorCode, Result};
use crate::model::*;
use crate::util::{new_id, now_ms};

const METADATA_AAD: &[u8] = b"chatapp-group-v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredMember {
    pub account_id: String,
    pub role: String,
    #[serde(default)]
    pub joined_at: i64,
    #[serde(default)]
    pub added_by: Option<String>,
}

pub(super) struct GroupRow {
    group_key: Option<Vec<u8>>,
    revision: i64,
    pub(super) name: String,
    pub(super) description: String,
    avatar_path: Option<String>,
    avatar_id: Option<String>,
    only_admins_send: bool,
    only_admins_edit: bool,
    disappearing_seconds: i64,
    hide_members: bool,
    member_names: String,
    pub(super) members: Vec<StoredMember>,
    left: bool,
    created_by: Option<String>,
    created_at: i64,
    pub(super) invite_secret: Option<String>,
    pub(super) invite_on: bool,
    pub(super) invite_approval: bool,
    join_requests: i64,
}

impl Inner {
    /// Whether `account` is an admin of the group: per the attested list when there is one
    /// (the server's roles are not trusted), else as the server lists it.
    pub(super) fn is_group_admin(&self, group_id: &str, account: &str) -> bool {
        if let Some(a) = self.attested(group_id) {
            return a.is_admin(account);
        }
        matches!(self.group_row(group_id), Ok(Some(g)) if g.members.iter().any(|m| m.account_id == account && m.role == "admin"))
    }

    /// Whether we left the group (or were removed). Unknown groups count as left.
    pub(super) fn group_left(&self, group_id: &str) -> bool {
        !matches!(self.group_row(group_id), Ok(Some(g)) if !g.left)
    }

    pub(super) fn group_row(&self, group_id: &str) -> Result<Option<GroupRow>> {
        Ok(self
            .db
            .borrow()
            .conn
            .query_row(
                "SELECT group_key, revision, name, description, avatar_path, avatar_id, only_admins_send, only_admins_edit,
                        members_json, left_group, created_by, created_at, disappearing_seconds, hide_members, member_names,
                        invite_secret, invite_on, invite_approval, join_requests
                 FROM groups WHERE group_id = ?1",
                [group_id],
                |r| {
                    let members: String = r.get(8)?;
                    Ok(GroupRow {
                        group_key: r.get(0)?,
                        revision: r.get(1)?,
                        name: r.get(2)?,
                        description: r.get(3)?,
                        avatar_path: r.get(4)?,
                        avatar_id: r.get(5)?,
                        only_admins_send: r.get(6)?,
                        only_admins_edit: r.get(7)?,
                        members: serde_json::from_str(&members).unwrap_or_default(),
                        left: r.get(9)?,
                        created_by: r.get(10)?,
                        created_at: r.get(11)?,
                        disappearing_seconds: r.get(12)?,
                        hide_members: r.get(13)?,
                        member_names: r.get(14)?,
                        invite_secret: r.get(15)?,
                        invite_on: r.get(16)?,
                        invite_approval: r.get(17)?,
                        join_requests: r.get(18)?,
                    })
                },
            )
            .optional()?)
    }

    pub fn group_member_ids(&self, group_id: &str) -> Result<Vec<String>> {
        Ok(self.group_row(group_id)?.map(|g| g.members.into_iter().map(|m| m.account_id).collect()).unwrap_or_default())
    }

    pub fn my_group_role(&self, group_id: &str) -> Result<Option<String>> {
        let me = self.me()?;
        Ok(self
            .group_row(group_id)?
            .filter(|g| !g.left)
            .and_then(|g| g.members.into_iter().find(|m| m.account_id == me).map(|m| m.role)))
    }

    pub(super) fn group_key(&self, group_id: &str) -> Result<[u8; 32]> {
        let k = self
            .group_row(group_id)?
            .and_then(|g| g.group_key)
            .ok_or_else(|| CoreError::internal("group key unknown"))?;
        crypto::key32(&k)
    }

    pub(super) fn ensure_group_row(&self, group_id: &str) -> Result<()> {
        self.db.borrow().conn.execute(
            "INSERT OR IGNORE INTO groups (group_id, distribution_id) VALUES (?1, ?2)",
            params![group_id, new_id()],
        )?;
        Ok(())
    }

    /// Stores the group key (from a pairwise control message or the vault).
    pub fn store_group_key(&self, group_id: &str, key: &[u8]) -> Result<bool> {
        crypto::key32(key)?;
        if !self.acceptable_group_key(group_id, key) {
            log::warn!("ignoring a key for v2 group {group_id} that is not its master key");
            return Ok(false);
        }
        self.ensure_group_row(group_id)?;
        let changed = self.db.borrow().conn.execute(
            "UPDATE groups SET group_key = ?2 WHERE group_id = ?1 AND (group_key IS NULL OR group_key != ?2)",
            params![group_id, key],
        )? > 0;
        if changed {
            self.vault_put_group(group_id);
        }
        Ok(changed)
    }

    /// Starts a new sender key chain for the group (after a member left or was removed).
    fn rotate_distribution(&self, group_id: &str) -> Result<()> {
        self.db
            .borrow()
            .conn
            .execute("UPDATE groups SET distribution_id = ?2 WHERE group_id = ?1", params![group_id, new_id()])?;
        self.group_devices.borrow_mut().remove(group_id);
        Ok(())
    }

    pub async fn sync_all_groups(&self) {
        let Ok(s) = self.session() else { return };
        if let Err(e) = self.bootstrap_attestations() {
            log::warn!("group attestation bootstrap: {e}");
        }
        #[derive(Deserialize)]
        struct Resp {
            groups: Vec<GroupRefDto>,
        }
        let Ok(r) = s.api.get::<Resp>("/v1/groups").await else { return };
        let server_ids: HashSet<String> = r.groups.iter().map(|g| g.group_id.clone()).collect();
        for g in r.groups {
            let local_rev = self.group_row(&g.group_id).ok().flatten().map(|l| l.revision).unwrap_or(-1);
            if local_rev < g.revision {
                if let Err(e) = self.sync_group(&g.group_id, Some(g.revision)).await {
                    log::warn!("group sync {}: {e}", g.group_id);
                }
            }
        }
        let local: Vec<String> = self
            .db
            .borrow()
            .conn
            .prepare("SELECT group_id FROM groups WHERE left_group = 0")
            .and_then(|mut st| st.query_map([], |r| r.get(0))?.collect())
            .unwrap_or_default();
        for g in local.into_iter().filter(|g| !server_ids.contains(g)) {
            let _ = self.sync_group(&g, None).await;
        }
    }

    /// Fetches the group from the server and updates local state.
    pub async fn sync_group(&self, group_id: &str, _revision: Option<i64>) -> Result<()> {
        if self.is_group_v2(group_id) {
            return self.sync_group_v2(group_id).await;
        }
        let s = self.session()?;
        let dto: GroupDto = match s.api.get(&format!("/v1/groups/{group_id}")).await {
            Ok(d) => d,
            Err(e) if e.code == ErrorCode::Forbidden || e.code == ErrorCode::NotFound => {
                if let Some(g) = self.group_row(group_id)? {
                    if !g.left {
                        self.db
                            .borrow()
                            .conn
                            .execute("UPDATE groups SET left_group = 1 WHERE group_id = ?1", [group_id])?;
                        self.group_devices.borrow_mut().remove(group_id);
                        self.emit_group(group_id);
                    }
                }
                return Ok(());
            }
            Err(e) => return Err(e),
        };
        let metadata = self
            .group_row(group_id)?
            .and_then(|g| g.group_key)
            .as_ref()
            .and_then(|k| crypto::key32(k).ok())
            .and_then(|k| crypto::aead_open(&k, &unb64(&dto.metadata).ok()?, METADATA_AAD).ok())
            .and_then(|p| serde_json::from_slice::<GroupMetadata>(&p).ok());
        self.apply_group_state(group_id, &dto, metadata).await
    }

    /// Stores a group state fetched from the server (v1, or decrypted v2 in groups_v2.rs).
    pub(super) async fn apply_group_state(&self, group_id: &str, dto: &GroupDto, metadata: Option<GroupMetadata>) -> Result<()> {
        let me = self.me()?;
        self.ensure_group_row(group_id)?;
        let old = self.group_row(group_id)?.expect("row exists");
        let members: Vec<StoredMember> = dto
            .members
            .iter()
            .map(|m| StoredMember {
                account_id: m.account_id.clone(),
                role: m.role.clone(),
                joined_at: m.joined_at,
                added_by: m.added_by.clone(),
            })
            .collect();
        let old_ids: HashSet<&str> = old.members.iter().map(|m| m.account_id.as_str()).collect();
        let new_ids: HashSet<&str> = members.iter().map(|m| m.account_id.as_str()).collect();
        if old_ids.iter().any(|id| !new_ids.contains(id)) {
            self.rotate_distribution(group_id)?;
        } else if new_ids.iter().any(|id| !old_ids.contains(id)) {
            self.group_devices.borrow_mut().remove(group_id);
        }
        let is_member = new_ids.contains(me.as_str());
        // A pending invite (engine/requests.rs): its avatar is not fetched until we join.
        let accepted = self.conversation_id_for_group(group_id)?.is_some_and(|c| self.conversation_accepted_id(&c));
        {
            let db = self.db.borrow();
            db.conn.execute(
                "UPDATE groups SET revision = ?2, members_json = ?3, only_admins_send = ?4, only_admins_edit = ?5,
                    left_group = ?6, created_by = ?7, created_at = ?8, disappearing_seconds = ?9, hide_members = ?10,
                    member_names = ?11, invite_on = ?12, invite_approval = ?13, join_requests = ?14 WHERE group_id = ?1",
                params![
                    group_id,
                    dto.revision,
                    serde_json::to_string(&members)?,
                    dto.settings.only_admins_send,
                    dto.settings.only_admins_edit,
                    !is_member,
                    dto.created_by,
                    dto.created_at,
                    dto.settings.disappearing_seconds,
                    dto.settings.hide_members,
                    dto.settings.member_names.as_deref().unwrap_or("number"),
                    dto.invite.is_some(),
                    dto.invite.as_ref().is_some_and(|i| i.approval),
                    dto.join_requests,
                ],
            )?;
        }
        if let Some(md) = metadata {
            self.db.borrow().conn.execute(
                "UPDATE groups SET name = ?2, description = ?3, invite_secret = ?4 WHERE group_id = ?1",
                params![
                    group_id,
                    crate::sanitize::clean_display_text(&md.name, 100),
                    md.description.chars().take(2000).collect::<String>(),
                    md.invite
                ],
            )?;
            match md.avatar {
                Some(_) if !accepted => {}
                Some(a) if old.avatar_id.as_deref() != Some(a.cdn_id.as_str()) || old.avatar_path.is_none() => {
                    match self.download_avatar(&a, group_id).await {
                        Ok(path) => {
                            self.db.borrow().conn.execute(
                                "UPDATE groups SET avatar_id = ?2, avatar_path = ?3 WHERE group_id = ?1",
                                params![group_id, a.cdn_id, path],
                            )?;
                        }
                        Err(e) => log::warn!("group avatar: {e}"),
                    }
                }
                Some(_) => {}
                None => {
                    self.db.borrow().conn.execute(
                        "UPDATE groups SET avatar_id = NULL, avatar_path = NULL WHERE group_id = ?1",
                        [group_id],
                    )?;
                }
            }
        }
        for m in &members {
            if m.account_id != me {
                let _ = self.ensure_contact(&m.account_id).await;
            }
        }
        let conv = self.create_group_conversation(group_id)?;
        if is_member {
            self.note_unverified_members(group_id, &conv)?;
        }
        // The group's timer is the rule for everyone (set by the admins).
        self.db.borrow().conn.execute(
            "UPDATE conversations SET disappearing_seconds = ?2 WHERE id = ?1 AND disappearing_seconds != ?2",
            params![conv, dto.settings.disappearing_seconds],
        )?;
        self.emit_group(group_id);
        Ok(())
    }

    /// The disappearing timer currently applied to a group conversation.
    fn group_timer(&self, conv_id: &str) -> i64 {
        self.db
            .borrow()
            .conn
            .query_row("SELECT disappearing_seconds FROM conversations WHERE id = ?1", [conv_id], |r| r.get(0))
            .unwrap_or(0)
    }

    /// Notes a change of the group timer in the conversation, once per new value. (Compared
    /// with the last value announced, not with the state before the sync: the server's
    /// group_updated push often syncs the group before the control message arrives.)
    fn note_group_timer(&self, conv_id: &str, actor: &str, id: &str) -> Result<()> {
        let now = self.group_timer(conv_id);
        let key = format!("group_timer_noticed:{conv_id}");
        let noticed: i64 = self.db.borrow().kv_get_json(&key)?.unwrap_or(0);
        if now == noticed {
            return Ok(());
        }
        self.db.borrow().kv_set_json(&key, &now)?;
        self.insert_system_message(conv_id, "disappearing_changed", Some(actor.to_string()), vec![], Some(now.to_string()), Some(format!("grptimer-{id}")), now_ms())?;
        self.expire_existing(conv_id, now)?;
        self.vault_put_conversation(conv_id);
        Ok(())
    }

    /// How a member is named inside a group (the "first_name" rule shows only the first word
    /// of their profile name, never the account number or a local nickname).
    pub fn member_name(&self, conv_id: &str, account_id: &str) -> String {
        let first_names: bool = self
            .db
            .borrow()
            .conn
            .query_row(
                "SELECT g.member_names = 'first_name' FROM conversations c JOIN groups g ON g.group_id = c.group_id WHERE c.id = ?1",
                [conv_id],
                |r| r.get(0),
            )
            .unwrap_or(false);
        if !first_names {
            return self.display_name(account_id);
        }
        first_name_of(self.contact_model(account_id).ok().and_then(|c| c.profile_name).as_deref())
    }

    /// "first_name" groups show profile names, which only members holding our profile key can
    /// decrypt: hand it to them (pairwise, end-to-end). `to` = None means every member, done
    /// once per group; new members get it as they are added.
    pub(super) fn share_profile_key_in_group(&self, group_id: &str, to: Option<Vec<String>>) -> Result<()> {
        let Some(g) = self.group_row(group_id)? else { return Ok(()) };
        if g.member_names != "first_name" || g.left {
            return Ok(());
        }
        // Not to the members of a group invite we did not accept (shared when we join).
        if !self.conversation_id_for_group(group_id)?.is_some_and(|c| self.conversation_accepted_id(&c)) {
            return Ok(());
        }
        let flag = format!("group_profile_key_shared:{group_id}");
        let targets = match to {
            Some(t) => t,
            None => {
                if self.db.borrow().kv_get_json::<bool>(&flag)?.unwrap_or(false) {
                    return Ok(());
                }
                self.db.borrow().kv_set_json(&flag, &true)?;
                g.members.into_iter().map(|m| m.account_id).collect()
            }
        };
        let me = self.me()?;
        let key = self.profile_key_b64()?;
        for account_id in targets.into_iter().filter(|a| *a != me && !self.is_blocked(a) && self.is_attested_member(group_id, a)) {
            self.enqueue(&Job::Account { account_id, content: Content::ProfileUpdated { profile_key: key.clone() } })?;
        }
        Ok(())
    }

    /// The group requires a profile name and we have none.
    pub fn require_profile_name_for(&self, member_names: &str) -> Result<()> {
        if member_names == "first_name" && self.get_profile()?.name.trim().is_empty() {
            return Err(CoreError::new(ErrorCode::ProfileNameRequired, "this group requires a profile name"));
        }
        Ok(())
    }

    pub(super) fn emit_group(&self, group_id: &str) {
        if let Ok(Some(conv)) = self.conversation_id_for_group(group_id) {
            if let Ok(info) = self.group_info(&conv) {
                self.emit("group_updated", &info);
            }
            self.emit_conversation(&conv);
        }
    }

    pub async fn handle_group_control(&self, sender: &str, ctrl: GroupControl) -> Result<()> {
        let key = unb64(&ctrl.group_key)?;
        let gid = ctrl.group_id.clone();
        let me = self.me()?;
        let had_key = self.group_row(&gid)?.and_then(|g| g.group_key).is_some();
        let was_member = self.is_attested_member(&gid, sender);
        // Membership and key changes are trusted only from attested admins (engine/attest.rs),
        // never from the server's list or from whoever claims a change.
        let may_rekey = self.apply_control_attestation(sender, &ctrl)?;
        let is_member = self.is_attested_member(&gid, sender);
        if may_rekey || (!had_key && is_member) {
            self.store_group_key(&gid, &key)?;
        }
        self.release_held_sender_keys(&gid).await?;
        // Being added by a stranger makes the group a pending invite (engine/requests.rs); by
        // ourselves, a contact or an accepted chat (or after we asked to join), it is accepted.
        // Decided before the sync so a trusted group's avatar is fetched right away.
        let adds_me = match &ctrl.change {
            GroupChange::Created => true,
            GroupChange::MembersAdded { accounts } => accounts.contains(&me),
            GroupChange::JoinedByLink => sender == me,
            _ => false,
        };
        if adds_me && (sender == me || is_member) {
            let asked_key = format!("join_requested:{gid}");
            let asked = self.db.borrow().kv_get(&asked_key)?.is_some();
            if sender == me || asked || self.trusts_account(sender) {
                let conv = self.create_group_conversation(&gid)?;
                self.mark_accepted(&conv)?;
                self.db.borrow().kv_delete(&asked_key)?;
            }
        }
        self.sync_group(&gid, Some(ctrl.revision)).await?;
        // The sender must be an attested member (or have been one, for Left).
        let allowed = sender == me || is_member || (was_member && matches!(ctrl.change, GroupChange::Left));
        if !allowed {
            log::warn!("ignoring group control for {gid} from non-member {sender}");
            return Ok(());
        }
        let Some(conv) = self.conversation_id_for_group(&gid)? else { return Ok(()) };
        if sender != me {
            self.note_group_timer(&conv, sender, &ctrl.id)?;
            match &ctrl.change {
                GroupChange::MembersAdded { accounts } if !accounts.contains(&me) => {
                    self.share_profile_key_in_group(&gid, Some(accounts.clone()))?
                }
                GroupChange::Created | GroupChange::MembersAdded { .. } | GroupChange::SettingsChanged => {
                    self.share_profile_key_in_group(&gid, None)?
                }
                GroupChange::JoinedByLink if is_member => {
                    self.share_profile_key_in_group(&gid, Some(vec![sender.to_string()]))?;
                    // Hand the newcomer the attested member list (it only has the server's).
                    if let Some(snapshot) = self.outgoing_snapshot(&gid) {
                        self.send_group_control_to(&gid, GroupChange::Snapshot, &[sender.to_string()], Some(snapshot), None)?;
                    }
                }
                _ => {}
            }
        }
        let actor = Some(sender.to_string());
        let (kind, targets, value) = match &ctrl.change {
            GroupChange::Created => ("group_created", vec![], None),
            GroupChange::MembersAdded { accounts } => ("group_member_added", accounts.clone(), None),
            GroupChange::MemberRemoved { account } => ("group_member_removed", vec![account.clone()], None),
            GroupChange::Left => ("group_member_left", vec![], None),
            GroupChange::JoinedByLink if is_member => ("group_member_joined_link", vec![], None),
            GroupChange::Renamed { name } => ("group_renamed", vec![], Some(crate::sanitize::clean_display_text(name, 100))),
            GroupChange::AvatarChanged => ("group_avatar_changed", vec![], None),
            GroupChange::AdminChanged { account, admin } => {
                (if *admin { "group_admin_granted" } else { "group_admin_revoked" }, vec![account.clone()], None)
            }
            GroupChange::JoinedByLink
            | GroupChange::DescriptionChanged
            | GroupChange::SettingsChanged
            | GroupChange::KeyRotated
            | GroupChange::Snapshot => return Ok(()),
        };
        self.insert_system_message(&conv, kind, actor, targets, value, Some(format!("grp-{}", ctrl.id)), now_ms())
    }

    /// Sends a group control message to all attested members (and our other devices). Members
    /// the server lists but no admin confirmed get nothing: the message carries the group key.
    pub(super) fn broadcast_group_control(&self, group_id: &str, change: GroupChange, extra: &[String]) -> Result<String> {
        self.broadcast_group_control_with(group_id, change, extra, None)
    }

    pub(super) fn broadcast_group_control_with(&self, group_id: &str, change: GroupChange, extra: &[String], join_proof: Option<String>) -> Result<String> {
        let me = self.me()?;
        let mut targets: Vec<String> = self.attested(group_id).map(|a| a.members.into_keys().collect()).unwrap_or_default();
        for e in extra {
            if !targets.contains(e) {
                targets.push(e.clone());
            }
        }
        targets.retain(|t| *t != me);
        let snapshot = self.outgoing_snapshot(group_id);
        let id = self.send_group_control_to(group_id, change, &targets, snapshot, join_proof)?;
        Ok(id)
    }

    /// Sends one control message to `targets` and to our own other devices.
    pub(super) fn send_group_control_to(
        &self,
        group_id: &str,
        change: GroupChange,
        targets: &[String],
        members: Option<Vec<crate::content::AttestedMember>>,
        join_proof: Option<String>,
    ) -> Result<String> {
        let g = self.group_row(group_id)?.ok_or_else(|| CoreError::not_found("group"))?;
        let key = g.group_key.ok_or_else(|| CoreError::internal("group key unknown"))?;
        let ctrl = GroupControl {
            group_id: group_id.to_string(),
            group_key: b64(&key),
            revision: g.revision,
            change,
            id: new_id(),
            members,
            join_proof,
        };
        let me = self.me()?;
        for t in targets.iter().filter(|t| **t != me) {
            self.enqueue(&Job::Account { account_id: t.clone(), content: Content::Group(ctrl.clone()) })?;
        }
        self.enqueue(&Job::OwnDevices { content: Content::Group(ctrl.clone()) })?;
        Ok(ctrl.id)
    }

    pub(super) fn seal_metadata(&self, key: &[u8; 32], md: &GroupMetadata) -> Result<String> {
        Ok(b64(crypto::aead_seal(key, &serde_json::to_vec(md)?, METADATA_AAD)?))
    }

    pub async fn create_group(&self, req: CreateGroupRequest) -> Result<Conversation> {
        self.create_group_with(req, None).await
    }

    /// Creates a private group (v2) whatever the server's default (fails if it has none).
    pub async fn create_group_v2(&self, req: CreateGroupRequest) -> Result<Conversation> {
        self.create_group_with(req, Some(2)).await
    }

    async fn create_group_with(&self, req: CreateGroupRequest, version: Option<u8>) -> Result<Conversation> {
        let v2 = self.create_as_v2(version)?;
        let name: String = req.name.trim().chars().take(100).collect();
        if name.is_empty() {
            return Err(CoreError::invalid("group name is required"));
        }
        let me = self.me()?;
        let mut settings = req.settings.clone().unwrap_or_default();
        // Groups we create start with our default timer unless the request chooses one.
        if settings.disappearing_seconds.is_none() {
            settings.disappearing_seconds = Some(self.default_disappearing_seconds()).filter(|d| *d > 0);
        }
        settings.validate()?;
        self.require_profile_name_for(settings.member_names.as_deref().unwrap_or("number"))?;
        let members: Vec<String> = req
            .member_account_ids
            .into_iter()
            .filter(|m| *m != me)
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        if members.len() > 255 {
            return Err(CoreError::invalid("too many members"));
        }
        let avatar = match &req.avatar_path {
            Some(p) if !p.is_empty() => Some(self.upload_avatar_from_path(p, "group-new").await?),
            _ => None,
        };
        let md = GroupMetadata {
            name: name.clone(),
            description: req.description.unwrap_or_default().trim().chars().take(2000).collect(),
            avatar: avatar.as_ref().map(|(_, a)| a.clone()),
            invite: None,
        };
        let (gid, key) = if v2 {
            self.create_group_v2_on_server(&md, &settings, &members).await?
        } else {
            let key = crypto::random_bytes::<32>();
            #[derive(Deserialize)]
            struct Created {
                group_id: String,
            }
            let created: Created = self
                .session()?
                .api
                .post(
                    "/v1/groups",
                    &json!({
                        "metadata": self.seal_metadata(&key, &md)?,
                        "avatar_id": avatar.as_ref().map(|(_, a)| a.cdn_id.clone()),
                        "members": members,
                        "settings": settings,
                    }),
                )
                .await?;
            (created.group_id, key)
        };
        self.store_group_key(&gid, &key)?;
        if let Some((path, a)) = &avatar {
            self.db.borrow().conn.execute(
                "UPDATE groups SET avatar_id = ?2, avatar_path = ?3 WHERE group_id = ?1",
                params![gid, a.cdn_id, path],
            )?;
        }
        self.attest_local(&gid, |a| {
            a.tofu = false;
            a.members = members.iter().map(|m| (m.clone(), "member".to_string())).collect();
            a.members.insert(me.clone(), "admin".into());
        })?;
        self.sync_group(&gid, None).await?;
        let rev = self.group_row(&gid)?.map(|g| g.revision).unwrap_or(1);
        self.attest_local(&gid, |a| a.revision = rev)?;
        let conv = self.create_group_conversation(&gid)?;
        self.mark_accepted(&conv)?;
        let id = self.broadcast_group_control(&gid, GroupChange::Created, &[])?;
        self.insert_system_message(&conv, "group_created", Some(me.clone()), vec![], Some(name), Some(format!("grp-{id}")), now_ms())?;
        self.note_group_timer(&conv, &me, &id)?;
        self.share_profile_key_in_group(&gid, None)?;
        self.vault_put_conversation(&conv);
        self.get_conversation(&conv)
    }

    pub(super) fn require_group_conv(&self, conv_id: &str) -> Result<String> {
        self.conversation_row(conv_id)?.group_id.ok_or_else(|| CoreError::invalid("not a group"))
    }

    pub(super) fn require_admin(&self, group_id: &str) -> Result<()> {
        if self.my_group_role(group_id)?.as_deref() != Some("admin") {
            return Err(CoreError::new(ErrorCode::Forbidden, "only admins can do this"));
        }
        Ok(())
    }

    pub async fn update_group(&self, req: UpdateGroupRequest) -> Result<GroupInfo> {
        let gid = self.require_group_conv(&req.conversation_id)?;
        let g = self.group_row(&gid)?.ok_or_else(|| CoreError::not_found("group"))?;
        let key = self.group_key(&gid)?;
        let api = self.session()?.api.clone();
        if let Some(settings) = &req.settings {
            settings.validate()?;
            self.require_admin(&gid)?;
            if let Some(n) = &settings.member_names {
                self.require_profile_name_for(n)?;
            }
            if self.is_group_v2(&gid) {
                self.set_settings_v2(&gid, settings).await?;
            } else {
                let _: serde_json::Value = api.put(&format!("/v1/groups/{gid}/settings"), settings).await?;
            }
            self.sync_group(&gid, None).await?;
            let id = self.broadcast_group_control(&gid, GroupChange::SettingsChanged, &[])?;
            self.note_group_timer(&req.conversation_id, &self.me()?, &id)?;
            self.share_profile_key_in_group(&gid, None)?;
            self.emit_conversation(&req.conversation_id);
        }
        let changes_metadata = req.name.is_some() || req.description.is_some() || req.avatar_path.is_some();
        if changes_metadata {
            if g.only_admins_edit {
                self.require_admin(&gid)?;
            }
            let current_avatar: Option<AvatarRef> = self.current_group_avatar(&gid, &key).await;
            let mut md = GroupMetadata {
                name: g.name.clone(),
                description: g.description.clone(),
                avatar: current_avatar,
                invite: g.invite_secret.clone(),
            };
            let mut change = None;
            if let Some(n) = &req.name {
                let n: String = n.trim().chars().take(100).collect();
                if n.is_empty() {
                    return Err(CoreError::invalid("group name is required"));
                }
                if n != md.name {
                    md.name = n.clone();
                    change = Some(GroupChange::Renamed { name: n });
                }
            }
            if let Some(d) = &req.description {
                md.description = d.trim().chars().take(2000).collect();
                change.get_or_insert(GroupChange::DescriptionChanged);
            }
            let mut new_avatar_path = None;
            if let Some(p) = &req.avatar_path {
                if p.is_empty() {
                    md.avatar = None;
                } else {
                    let (path, a) = self.upload_avatar_from_path(p, &gid).await?;
                    md.avatar = Some(a);
                    new_avatar_path = Some(path);
                }
                change = Some(GroupChange::AvatarChanged);
            }
            if self.is_group_v2(&gid) {
                self.set_attributes_v2(&gid, &md).await?;
            } else {
                let _: serde_json::Value = api
                    .put(
                        &format!("/v1/groups/{gid}/metadata"),
                        &json!({ "metadata": self.seal_metadata(&key, &md)?, "avatar_id": md.avatar.as_ref().map(|a| a.cdn_id.clone()) }),
                    )
                    .await?;
            }
            if let (Some(path), Some(a)) = (new_avatar_path, &md.avatar) {
                self.db.borrow().conn.execute(
                    "UPDATE groups SET avatar_id = ?2, avatar_path = ?3 WHERE group_id = ?1",
                    params![gid, a.cdn_id, path],
                )?;
            }
            self.sync_group(&gid, None).await?;
            if let Some(change) = change {
                let value = if let GroupChange::Renamed { name } = &change { Some(name.clone()) } else { None };
                let kind = match &change {
                    GroupChange::Renamed { .. } => Some("group_renamed"),
                    GroupChange::AvatarChanged => Some("group_avatar_changed"),
                    _ => None,
                };
                let id = self.broadcast_group_control(&gid, change, &[])?;
                if let Some(kind) = kind {
                    self.insert_system_message(&req.conversation_id, kind, Some(self.me()?), vec![], value, Some(format!("grp-{id}")), now_ms())?;
                }
            }
        } else {
            self.sync_group(&gid, None).await?;
        }
        self.group_info(&req.conversation_id)
    }

    /// Decrypts the avatar reference currently stored in the server metadata.
    pub(super) async fn current_group_avatar(&self, group_id: &str, key: &[u8; 32]) -> Option<AvatarRef> {
        if self.is_group_v2(group_id) {
            return self.current_attributes_v2(group_id).await?.avatar;
        }
        let dto: GroupDto = self.session().ok()?.api.get(&format!("/v1/groups/{group_id}")).await.ok()?;
        let plain = crypto::aead_open(key, &unb64(&dto.metadata).ok()?, METADATA_AAD).ok()?;
        serde_json::from_slice::<GroupMetadata>(&plain).ok()?.avatar
    }

    pub async fn add_group_members(&self, conv_id: &str, account_ids: Vec<String>) -> Result<GroupInfo> {
        let gid = self.require_group_conv(conv_id)?;
        self.require_admin(&gid)?;
        let existing: HashSet<String> = self.group_member_ids(&gid)?.into_iter().collect();
        let new: Vec<String> = account_ids.into_iter().filter(|a| !existing.contains(a)).collect();
        if new.is_empty() {
            return self.group_info(conv_id);
        }
        if self.is_group_v2(&gid) {
            self.add_members_v2(&gid, &new).await?;
        } else {
            let _: serde_json::Value = self
                .session()?
                .api
                .post(&format!("/v1/groups/{gid}/members"), &json!({ "account_ids": new }))
                .await?;
        }
        self.sync_group(&gid, None).await?;
        self.attest_added(&gid, &new)?;
        let id = self.broadcast_group_control(&gid, GroupChange::MembersAdded { accounts: new.clone() }, &[])?;
        self.share_profile_key_in_group(&gid, Some(new.clone()))?;
        self.insert_system_message(conv_id, "group_member_added", Some(self.me()?), new, None, Some(format!("grp-{id}")), now_ms())?;
        self.group_info(conv_id)
    }

    /// We (an admin) added these accounts: they become attested at the synced revision.
    pub(super) fn attest_added(&self, gid: &str, accounts: &[String]) -> Result<()> {
        let rev = self.group_row(gid)?.map(|g| g.revision).unwrap_or(0);
        self.attest_local(gid, |a| {
            a.tofu = false;
            a.revision = rev;
            for acc in accounts {
                a.members.entry(acc.clone()).or_insert_with(|| "member".into());
            }
        })
    }

    pub async fn remove_group_member(&self, conv_id: &str, account_id: &str) -> Result<GroupInfo> {
        let gid = self.require_group_conv(conv_id)?;
        let me = self.me()?;
        if account_id == me {
            self.leave_group(conv_id).await?;
            return self.group_info(conv_id);
        }
        self.require_admin(&gid)?;
        if self.is_group_v2(&gid) {
            return self.remove_group_member_v2(conv_id, &gid, account_id).await;
        }
        let api = self.session()?.api.clone();
        api.delete::<()>(&format!("/v1/groups/{gid}/members/{account_id}"), None).await?;
        // Rotate the metadata key so the removed member cannot read future metadata.
        let g = self.group_row(&gid)?.ok_or_else(|| CoreError::not_found("group"))?;
        let old_key = self.group_key(&gid)?;
        let avatar = self.current_group_avatar(&gid, &old_key).await;
        let new_key = crypto::random_bytes::<32>();
        // The removed member may know the invite link: it is replaced by a new one.
        let new_secret = g.invite_on.then(super::invites::new_invite_secret);
        let md = GroupMetadata { name: g.name, description: g.description, avatar, invite: new_secret.clone().or(g.invite_secret) };
        let _: serde_json::Value = api
            .put(
                &format!("/v1/groups/{gid}/metadata"),
                &json!({ "metadata": self.seal_metadata(&new_key, &md)?, "avatar_id": md.avatar.as_ref().map(|a| a.cdn_id.clone()) }),
            )
            .await?;
        self.store_group_key(&gid, &new_key)?;
        if let Some(secret) = &new_secret {
            self.db.borrow().conn.execute("UPDATE groups SET invite_secret = ?2 WHERE group_id = ?1", params![gid, secret])?;
            self.publish_invite(&gid, secret, g.invite_approval).await?;
        }
        self.sync_group(&gid, None).await?;
        self.rotate_distribution(&gid)?;
        let rev = self.group_row(&gid)?.map(|g| g.revision).unwrap_or(0);
        self.attest_local(&gid, |a| {
            a.tofu = false;
            a.revision = rev;
            a.members.remove(account_id);
        })?;
        let id = self.broadcast_group_control(&gid, GroupChange::MemberRemoved { account: account_id.to_string() }, &[])?;
        self.insert_system_message(conv_id, "group_member_removed", Some(me), vec![account_id.to_string()], None, Some(format!("grp-{id}")), now_ms())?;
        self.group_info(conv_id)
    }

    pub async fn set_group_admin(&self, conv_id: &str, account_id: &str, admin: bool) -> Result<GroupInfo> {
        let gid = self.require_group_conv(conv_id)?;
        self.require_admin(&gid)?;
        if self.is_group_v2(&gid) {
            self.set_role_v2(&gid, account_id, admin).await?;
        } else {
            let _: serde_json::Value = self
                .session()?
                .api
                .put(
                    &format!("/v1/groups/{gid}/members/{account_id}/role"),
                    &json!({ "role": if admin { "admin" } else { "member" } }),
                )
                .await?;
        }
        self.sync_group(&gid, None).await?;
        let rev = self.group_row(&gid)?.map(|g| g.revision).unwrap_or(0);
        self.attest_local(&gid, |a| {
            a.revision = rev;
            if let Some(role) = a.members.get_mut(account_id) {
                *role = if admin { "admin".into() } else { "member".into() };
            }
        })?;
        let id = self.broadcast_group_control(&gid, GroupChange::AdminChanged { account: account_id.to_string(), admin }, &[])?;
        let kind = if admin { "group_admin_granted" } else { "group_admin_revoked" };
        self.insert_system_message(conv_id, kind, Some(self.me()?), vec![account_id.to_string()], None, Some(format!("grp-{id}")), now_ms())?;
        self.group_info(conv_id)
    }

    pub async fn leave_group(&self, conv_id: &str) -> Result<()> {
        let gid = self.require_group_conv(conv_id)?;
        let me = self.me()?;
        // Tell the others first (pairwise messages do not require membership).
        let id = self.broadcast_group_control(&gid, GroupChange::Left, &[])?;
        if self.is_group_v2(&gid) {
            self.remove_member_v2(&gid, &me).await?;
        } else {
            self.session()?.api.delete::<()>(&format!("/v1/groups/{gid}/members/{me}"), None).await?;
        }
        self.db.borrow().conn.execute("UPDATE groups SET left_group = 1 WHERE group_id = ?1", [&gid])?;
        self.group_devices.borrow_mut().remove(&gid);
        self.insert_system_message(conv_id, "group_member_left", Some(me), vec![], None, Some(format!("grp-{id}")), now_ms())?;
        self.emit_group(&gid);
        Ok(())
    }

    pub fn group_info(&self, conv_id: &str) -> Result<GroupInfo> {
        let gid = self.require_group_conv(conv_id)?;
        let g = self.group_row(&gid)?.ok_or_else(|| CoreError::not_found("group"))?;
        let me = self.me()?;
        let my_role = if g.left { None } else { g.members.iter().find(|m| m.account_id == me).map(|m| m.role.clone()) };
        let is_admin = my_role.as_deref() == Some("admin");
        // Hidden list: members only learn how many there are (admins still manage them).
        let members_hidden = g.hide_members && !is_admin;
        let first_names = g.member_names == "first_name";
        let mut members = Vec::new();
        for m in g.members.iter().filter(|m| !members_hidden || m.account_id == me) {
            let c = self.contact_model(&m.account_id)?;
            let is_me = m.account_id == me;
            members.push(GroupMember {
                account_id: m.account_id.clone(),
                number: if is_me {
                    self.session()?.account.number.clone()
                } else if first_names {
                    String::new()
                } else {
                    c.number
                },
                display_name: if is_me {
                    "You".into()
                } else if first_names {
                    first_name_of(c.profile_name.as_deref())
                } else {
                    c.display_name
                },
                avatar_path: if is_me { self.get_profile()?.avatar_path } else { c.avatar_path },
                role: m.role.clone(),
                is_me,
                verified: is_me || self.is_attested_member(&gid, &m.account_id),
            });
        }
        members.sort_by_key(|m| (!m.is_me, m.role != "admin", m.display_name.to_lowercase()));
        Ok(GroupInfo {
            conversation_id: conv_id.to_string(),
            group_id: gid,
            name: if g.name.is_empty() { "Group".into() } else { g.name },
            description: g.description,
            avatar_path: g.avatar_path,
            members,
            member_count: g.members.len(),
            members_hidden,
            my_role,
            settings: GroupSettings {
                only_admins_send: g.only_admins_send,
                only_admins_edit: g.only_admins_edit,
                disappearing_seconds: g.disappearing_seconds,
                hide_members: g.hide_members,
                member_names: g.member_names,
            },
            created_by: g.created_by,
            created_at: g.created_at,
            invite_link: if is_admin && g.invite_on {
                g.invite_secret.as_deref().map(super::invites::invite_link)
            } else {
                None
            },
            invite_approval: g.invite_approval,
            join_requests: if is_admin { g.join_requests } else { 0 },
        })
    }
}

/// First word of a profile name ("Member" when there is none).
fn first_name_of(profile_name: Option<&str>) -> String {
    profile_name
        .and_then(|n| n.split_whitespace().next())
        .map(|n| n.chars().take(40).collect())
        .unwrap_or_else(|| "Member".into())
}
