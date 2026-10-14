//! End-to-end attested group membership.
//!
//! The server stores group membership and could add an account of its own (a "ghost") to a
//! group. Clients therefore keep their own membership list per group, changed only by group
//! control messages that arrive over pairwise Signal sessions:
//! * a snapshot of all members from an admin of the current attested list (or, for a group we
//!   learn about, from an admin listed in the snapshot that also lists us);
//! * JoinedByLink with a proof derived from the invite link secret (unknown to the server);
//! * Left from the member who leaves.
//! Sender keys, the metadata key and profile keys go only to attested members, and group
//! content from anyone else is dropped. A ghost still receives the (server-routed) group
//! ciphertext but can never decrypt it; members see it flagged as unverified.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::Inner;
use crate::content::{AttestedMember, GroupChange, GroupControl};
use crate::crypto::{self, b64, unb64};
use crate::error::Result;
use crate::util::now_ms;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Attested {
    /// Group revision of the snapshot this state is based on (older snapshots are ignored).
    pub revision: i64,
    /// account id -> role ("admin" | "member").
    pub members: BTreeMap<String, String>,
    /// Taken over from the server's list without an end-to-end snapshot (groups that existed
    /// before attestation, or a group just joined by link until an admin sends a snapshot).
    #[serde(default)]
    pub tofu: bool,
}

impl Attested {
    pub fn is_member(&self, account: &str) -> bool {
        self.members.contains_key(account)
    }
    pub fn is_admin(&self, account: &str) -> bool {
        self.members.get(account).map(String::as_str) == Some("admin")
    }
    pub fn snapshot(&self) -> Vec<AttestedMember> {
        self.members.iter().map(|(a, r)| AttestedMember { account: a.clone(), role: r.clone() }).collect()
    }
    fn from_snapshot(revision: i64, members: &[AttestedMember]) -> Attested {
        let members = members
            .iter()
            .map(|m| (m.account.clone(), if m.role == "admin" { "admin".to_string() } else { "member".to_string() }))
            .collect();
        Attested { revision, members, tofu: false }
    }
}

const BOOTSTRAP_FLAG: &str = "group_attest_bootstrapped";

fn key(group_id: &str) -> String {
    format!("group_attested:{group_id}")
}

impl Inner {
    pub fn attested(&self, group_id: &str) -> Option<Attested> {
        self.db.borrow().kv_get_json::<Attested>(&key(group_id)).ok().flatten()
    }

    pub(super) fn set_attested(&self, group_id: &str, a: &Attested) -> Result<()> {
        if self.attested(group_id).as_ref() == Some(a) {
            return Ok(());
        }
        self.db.borrow().kv_set_json(&key(group_id), a)?;
        // Our other devices (and newly linked ones) share the list through the vault.
        self.vault_put_group(group_id);
        Ok(())
    }

    /// Whether `account` is an end-to-end attested member (we always count ourselves).
    pub fn is_attested_member(&self, group_id: &str, account: &str) -> bool {
        account == self.me().unwrap_or_default() || self.attested(group_id).is_some_and(|a| a.is_member(account))
    }

    pub fn is_attested_admin(&self, group_id: &str, account: &str) -> bool {
        self.attested(group_id).is_some_and(|a| a.is_admin(account))
    }

    /// Server members that no admin has vouched for.
    pub fn unverified_members(&self, group_id: &str) -> Vec<String> {
        let Some(a) = self.attested(group_id) else { return Vec::new() };
        let me = self.me().unwrap_or_default();
        self.group_member_ids(group_id)
            .unwrap_or_default()
            .into_iter()
            .filter(|m| *m != me && !a.is_member(m))
            .collect()
    }

    /// One-time migration: groups that existed before attestation start from the server's
    /// list (trust on first use).
    pub(super) fn bootstrap_attestations(&self) -> Result<()> {
        if self.db.borrow().kv_get_json::<bool>(BOOTSTRAP_FLAG)?.unwrap_or(false) {
            return Ok(());
        }
        let groups: Vec<String> = self
            .db
            .borrow()
            .conn
            .prepare("SELECT group_id FROM groups WHERE left_group = 0")?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        for g in groups {
            if self.attested(&g).is_none() {
                self.attest_from_server(&g)?;
            }
        }
        self.db.borrow().kv_set_json(BOOTSTRAP_FLAG, &true)
    }

    /// Takes the server's current list as attested (TOFU). Only for the cases documented on
    /// `Attested::tofu`.
    pub(super) fn attest_from_server(&self, group_id: &str) -> Result<()> {
        let Some(g) = self.group_snapshot_from_server_row(group_id)? else { return Ok(()) };
        self.set_attested(group_id, &Attested { tofu: true, ..g })
    }

    fn group_snapshot_from_server_row(&self, group_id: &str) -> Result<Option<Attested>> {
        let row: Option<(i64, String)> = {
            use rusqlite::OptionalExtension;
            self.db
                .borrow()
                .conn
                .query_row("SELECT revision, members_json FROM groups WHERE group_id = ?1", [group_id], |r| Ok((r.get(0)?, r.get(1)?)))
                .optional()?
        };
        let Some((revision, json)) = row else { return Ok(None) };
        let members: Vec<super::groups::StoredMember> = serde_json::from_str(&json).unwrap_or_default();
        Ok(Some(Attested {
            revision,
            members: members.into_iter().map(|m| (m.account_id, m.role)).collect(),
            tofu: false,
        }))
    }

    /// Our own attested change (we are an admin, or we joined/left): applied locally before
    /// the control message is sent.
    pub(super) fn attest_local(&self, group_id: &str, f: impl FnOnce(&mut Attested)) -> Result<()> {
        let mut a = self.attested(group_id).unwrap_or_default();
        f(&mut a);
        self.promote_if_no_admin(group_id, &mut a);
        self.set_attested(group_id, &a)
    }

    /// Like the server: when the last admin is gone, the longest-standing remaining member
    /// becomes admin (decided from the server's join order, among attested members only).
    fn promote_if_no_admin(&self, group_id: &str, a: &mut Attested) {
        if a.members.is_empty() || a.members.values().any(|r| r == "admin") {
            return;
        }
        let order: Vec<String> = self
            .group_row(group_id)
            .ok()
            .flatten()
            .map(|g| {
                let mut m = g.members.clone();
                m.sort_by_key(|m| m.joined_at);
                m.into_iter().map(|m| m.account_id).collect()
            })
            .unwrap_or_default();
        let next = order.into_iter().find(|id| a.is_member(id)).or_else(|| a.members.keys().next().cloned());
        if let Some(id) = next {
            a.members.insert(id, "admin".into());
        }
    }

    /// The snapshot we attach to control messages, when we are an attested admin.
    pub(super) fn outgoing_snapshot(&self, group_id: &str) -> Option<Vec<AttestedMember>> {
        let a = self.attested(group_id)?;
        a.is_admin(&self.me().ok()?).then(|| a.snapshot())
    }

    /// Proof for JoinedByLink (we hold the invite secret from the link).
    pub(super) fn join_proof_for(&self, group_id: &str, secret: &[u8; 16]) -> Result<String> {
        Ok(b64(crypto::join_proof(secret, group_id, &self.me()?)))
    }

    fn valid_join_proof(&self, group_id: &str, account: &str, proof: &str) -> bool {
        let Some(secret) = self.group_row(group_id).ok().flatten().and_then(|g| g.invite_secret) else { return false };
        let Ok(secret) = super::invites::parse_invite_link(&secret) else { return false };
        let expected = crypto::join_proof(&secret, group_id, account);
        unb64(proof).is_ok_and(|p| crypto::ct_eq(&p, &expected))
    }

    /// Applies a received control message to the attested list. Returns whether the sender
    /// may change the group key (an attested admin, judged before this message).
    pub(super) fn apply_control_attestation(&self, sender: &str, ctrl: &GroupControl) -> Result<bool> {
        let gid = &ctrl.group_id;
        let me = self.me()?;
        let before = self.attested(gid);
        let sender_admin = before.as_ref().is_some_and(|a| a.is_admin(sender));
        let mut next = before.clone();
        if let Some(snap) = &ctrl.members {
            let lists_sender_as_admin = snap.iter().any(|m| m.account == sender && m.role == "admin");
            let lists_me = snap.iter().any(|m| m.account == me);
            let acceptable = match &before {
                // An attested admin's newer snapshot is authoritative.
                Some(a) if !a.tofu => sender_admin && ctrl.revision >= a.revision,
                // TOFU state: an admin of that state replaces it with a real snapshot.
                Some(a) => a.is_admin(sender) && lists_me && ctrl.revision >= a.revision,
                // A group we do not know yet: first contact is trust on first use (the admin
                // who adds us, authenticated by Signal), like identity keys.
                None => lists_sender_as_admin && lists_me,
            };
            if acceptable {
                next = Some(Attested::from_snapshot(ctrl.revision, snap));
            } else {
                log::warn!("ignoring group snapshot for {gid} from {sender} (not an attested admin)");
            }
        }
        match &ctrl.change {
            GroupChange::JoinedByLink => {
                let valid = ctrl.join_proof.as_deref().is_some_and(|p| self.valid_join_proof(gid, sender, p));
                if let (true, Some(a)) = (valid, next.as_mut()) {
                    a.members.entry(sender.to_string()).or_insert_with(|| "member".into());
                } else if !valid {
                    log::warn!("JoinedByLink in {gid} from {sender} without a valid link proof");
                }
            }
            GroupChange::Left => {
                if let Some(a) = next.as_mut() {
                    a.members.remove(sender);
                }
            }
            _ => {}
        }
        if let Some(mut a) = next {
            self.promote_if_no_admin(gid, &mut a);
            if Some(&a) != before.as_ref() {
                self.set_attested(gid, &a)?;
            }
        }
        // Key changes: only an attested admin (as known before this message), or the admin
        // whose snapshot just introduced us to the group.
        let introduced = before.is_none() && self.attested(gid).is_some_and(|a| a.is_admin(sender));
        Ok(sender_admin || introduced)
    }

    /// Keeps a sender key from a not (yet) attested member.
    pub(super) fn hold_sender_key(&self, group_id: &str, account: &str, device: u32, skdm: &str) -> Result<()> {
        let k = format!("held_skdm:{group_id}");
        let mut held: Vec<(String, u32, String)> = self.db.borrow().kv_get_json(&k)?.unwrap_or_default();
        held.retain(|(a, d, _)| !(a == account && *d == device));
        held.push((account.to_string(), device, skdm.to_string()));
        // Bounded: a server could flood us with keys from accounts of its own.
        if held.len() > 256 {
            held.drain(..held.len() - 256);
        }
        self.db.borrow().kv_set_json(&k, &held)
    }

    /// Processes held sender keys of members that are attested now.
    pub(super) async fn release_held_sender_keys(&self, group_id: &str) -> Result<()> {
        use libsignal_protocol::{process_sender_key_distribution_message, DeviceId, ProtocolAddress, SenderKeyDistributionMessage};
        let k = format!("held_skdm:{group_id}");
        let held: Vec<(String, u32, String)> = self.db.borrow().kv_get_json(&k)?.unwrap_or_default();
        if held.is_empty() {
            return Ok(());
        }
        let (ready, keep): (Vec<_>, Vec<_>) = held.into_iter().partition(|(a, _, _)| self.is_attested_member(group_id, a));
        self.db.borrow().kv_set_json(&k, &keep)?;
        for (account, device, skdm) in ready {
            let Ok(dev) = DeviceId::new(device as u8) else { continue };
            let addr = ProtocolAddress::new(account.clone(), dev);
            let Ok(msg) = unb64(&skdm).map_err(|_| ()).and_then(|b| SenderKeyDistributionMessage::try_from(&b[..]).map_err(|_| ())) else { continue };
            let mut st = self.signal_store()?;
            if let Err(e) = process_sender_key_distribution_message(&addr, &msg, &mut st).await {
                log::warn!("held sender key from {account}: {e}");
            }
        }
        Ok(())
    }

    /// Flags (once) server-side members nobody vouched for, in the conversation.
    pub(super) fn note_unverified_members(&self, group_id: &str, conv_id: &str) -> Result<()> {
        for account in self.unverified_members(group_id) {
            let flag = format!("group_unverified_noticed:{group_id}:{account}");
            if self.db.borrow().kv_get_json::<bool>(&flag)?.unwrap_or(false) {
                continue;
            }
            self.db.borrow().kv_set_json(&flag, &true)?;
            log::warn!("group {group_id}: {account} is listed by the server but no admin confirmed it");
            self.insert_system_message(conv_id, "group_member_unverified", None, vec![account.clone()], None, Some(format!("unverified-{group_id}-{account}")), now_ms())?;
        }
        Ok(())
    }
}

#[cfg(feature = "insecure-test-hooks")]
impl Inner {
    /// TEST ONLY: behave like a modified client that trusts the server's member list (to show
    /// that honest members never hand such a client their keys).
    pub async fn insecure_trust_server_group(&self, group_id: &str) -> Result<()> {
        self.sync_group(group_id, None).await?;
        let mut a = self.group_snapshot_from_server_row(group_id)?.unwrap_or_default();
        a.tofu = false;
        self.db.borrow().kv_set_json(&key(group_id), &a)?;
        self.release_held_sender_keys(group_id).await
    }
}
