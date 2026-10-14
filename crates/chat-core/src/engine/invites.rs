//! Group invite links.
//!
//! A link carries a random 16-byte secret S (`chatapp://join/<base64url S>`). Two keys are
//! derived from it: a token, sent to the server (which stores only its hash), and a blob key,
//! which never leaves the clients. The blob, stored by the server and returned to anyone who
//! presents the token, is encrypted with the blob key and previews the group (name,
//! description); when no approval is needed it also holds the group key, so the new member
//! can read the group metadata right away. With approval, the request carries the
//! requester's name and profile key encrypted with the blob key, and the approving admin
//! delivers the group key pairwise (end-to-end) like any added member.
//!
//! S lives in the encrypted group metadata, so every admin's device can show and update the
//! link. Removing a member replaces the link (the removed member knew it).

use std::rc::Rc;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::Inner;
use crate::content::{AvatarRef, GroupChange};
use crate::crypto::{self, b64, unb64};
use crate::error::{CoreError, ErrorCode, Result};
use crate::model::GroupInfo;
use crate::util::now_ms;

const LINK_PREFIX: &str = "chatapp://join/";
const BLOB_AAD: &[u8] = b"chatapp-invite-blob-v1";
const INTRO_AAD: &[u8] = b"chatapp-invite-intro-v1";

/// What the blob reveals to whoever holds the link.
#[derive(Serialize, Deserialize)]
struct InviteBlob {
    group_id: String,
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    avatar: Option<AvatarRef>,
    /// Only when joining needs no approval.
    #[serde(default)]
    group_key: Option<String>,
}

/// The requester introduces themselves to the admins (approval links).
#[derive(Serialize, Deserialize)]
struct Intro {
    name: String,
    profile_key: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct InvitePreview {
    pub group_id: String,
    pub name: String,
    pub description: String,
    pub member_count: i64,
    /// Joining needs an admin's approval.
    pub approval: bool,
    pub is_member: bool,
    /// We already asked to join.
    pub requested: bool,
    /// An admin removed us (or rejected our request): joining by link is refused.
    pub banned: bool,
    /// The group's conversation, when we are a member.
    pub conversation_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct JoinResult {
    /// "joined", "requested" or "member" (already in the group).
    pub status: String,
    pub conversation_id: Option<String>,
    pub group_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct JoinRequest {
    pub account_id: String,
    /// The name the requester gave (their profile name).
    pub name: String,
    pub avatar_path: Option<String>,
    pub requested_at: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SetInviteRequest {
    pub conversation_id: String,
    /// false turns the link off.
    pub enabled: bool,
    /// Joining needs an admin's approval (unchanged when omitted).
    #[serde(default)]
    pub approval: Option<bool>,
    /// Replace the link with a new one (the old one stops working).
    #[serde(default)]
    pub reset: bool,
}

pub fn new_invite_secret() -> String {
    URL_SAFE_NO_PAD.encode(crypto::random_bytes::<16>())
}

pub fn invite_link(secret: &str) -> String {
    format!("{LINK_PREFIX}{secret}")
}

/// The secret of a link (also accepted: the bare secret, or the link inside other text).
pub fn parse_invite_link(text: &str) -> Result<[u8; 16]> {
    let t = text.trim();
    let candidate = match t.find("join/") {
        Some(i) => &t[i + 5..],
        None => t,
    };
    let secret: String = candidate.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
    let bytes = URL_SAFE_NO_PAD.decode(secret.as_bytes()).map_err(|_| invalid_link())?;
    bytes.try_into().map_err(|_| invalid_link())
}

fn invalid_link() -> CoreError {
    CoreError::invalid("this is not a group invite link")
}

fn decode_secret(secret: &str) -> Result<[u8; 16]> {
    parse_invite_link(secret)
}

impl Inner {
    /// Uploads the link's token and blob (creating, resetting or updating the invite).
    pub(super) async fn publish_invite(&self, gid: &str, secret: &str, approval: bool) -> Result<()> {
        let (token, blob_key) = crypto::invite_keys(&decode_secret(secret)?);
        let g = self.group_row(gid)?.ok_or_else(|| CoreError::not_found("group"))?;
        let key = self.group_key(gid)?;
        let blob = InviteBlob {
            group_id: gid.to_string(),
            name: g.name.clone(),
            description: g.description.clone(),
            avatar: self.current_group_avatar(gid, &key).await,
            group_key: (!approval).then(|| b64(key)),
        };
        let sealed = crypto::aead_seal(&blob_key, &serde_json::to_vec(&blob)?, BLOB_AAD)?;
        let _: serde_json::Value = self
            .session()?
            .api
            .put(&format!("/v1/groups/{gid}/invite"), &json!({ "token": b64(token), "approval": approval, "blob": b64(sealed) }))
            .await?;
        Ok(())
    }

    /// Re-seals the group metadata with the invite secret (or without it).
    async fn put_invite_secret(&self, gid: &str, secret: Option<String>) -> Result<()> {
        let g = self.group_row(gid)?.ok_or_else(|| CoreError::not_found("group"))?;
        let key = self.group_key(gid)?;
        let avatar = self.current_group_avatar(gid, &key).await;
        let md = crate::content::GroupMetadata { name: g.name, description: g.description, avatar, invite: secret.clone() };
        let _: serde_json::Value = self
            .session()?
            .api
            .put(
                &format!("/v1/groups/{gid}/metadata"),
                &json!({ "metadata": self.seal_metadata(&key, &md)?, "avatar_id": md.avatar.as_ref().map(|a| a.cdn_id.clone()) }),
            )
            .await?;
        self.db.borrow().conn.execute("UPDATE groups SET invite_secret = ?2 WHERE group_id = ?1", params![gid, secret])?;
        Ok(())
    }

    /// Creates, resets, updates or turns off the group's invite link (admins).
    pub async fn set_group_invite(&self, req: SetInviteRequest) -> Result<GroupInfo> {
        let gid = self.require_group_conv(&req.conversation_id)?;
        self.require_admin(&gid)?;
        if self.is_group_v2(&gid) {
            // Invite links for private groups are stage 5 of docs/ZKGROUP-DESIGN.md.
            return Err(CoreError::new(crate::error::ErrorCode::Forbidden, "invite links are not available for this group yet"));
        }
        let g = self.group_row(&gid)?.ok_or_else(|| CoreError::not_found("group"))?;
        if !req.enabled {
            if g.invite_on {
                self.session()?.api.delete::<()>(&format!("/v1/groups/{gid}/invite"), None).await?;
            }
            if g.invite_secret.is_some() {
                self.put_invite_secret(&gid, None).await?;
            }
        } else {
            let approval = req.approval.unwrap_or(if g.invite_on { g.invite_approval } else { false });
            let secret = match (&g.invite_secret, req.reset || !g.invite_on) {
                (Some(s), false) => s.clone(),
                _ => new_invite_secret(),
            };
            // Metadata first: if publishing fails, the next attempt replaces the secret anyway.
            if g.invite_secret.as_deref() != Some(secret.as_str()) {
                self.put_invite_secret(&gid, Some(secret.clone())).await?;
            }
            self.publish_invite(&gid, &secret, approval).await?;
        }
        self.sync_group(&gid, None).await?;
        self.group_info(&req.conversation_id)
    }

    async fn lookup_invite(&self, secret: &[u8; 16]) -> Result<(serde_json::Value, InviteBlob, [u8; 32])> {
        let (token, blob_key) = crypto::invite_keys(secret);
        let res: serde_json::Value = self.session()?.api.post("/v1/invites/lookup", &json!({ "token": b64(token) })).await?;
        let sealed = unb64(res["blob"].as_str().unwrap_or_default())?;
        let plain = crypto::aead_open(&blob_key, &sealed, BLOB_AAD).map_err(|_| invalid_link())?;
        let mut blob: InviteBlob = serde_json::from_slice(&plain)?;
        blob.name = crate::sanitize::clean_display_text(&blob.name, 100);
        if Some(blob.group_id.as_str()) != res["group_id"].as_str() {
            return Err(invalid_link());
        }
        Ok((res, blob, token))
    }

    /// What a link leads to, before joining.
    pub async fn preview_group_invite(&self, link: &str) -> Result<InvitePreview> {
        let secret = parse_invite_link(link)?;
        let (res, blob, _) = self.lookup_invite(&secret).await?;
        let is_member = res["is_member"].as_bool().unwrap_or(false);
        let conversation_id = if is_member { self.conversation_id_for_group(&blob.group_id)? } else { None };
        Ok(InvitePreview {
            group_id: blob.group_id,
            name: blob.name,
            description: blob.description,
            member_count: res["member_count"].as_i64().unwrap_or(0),
            approval: res["approval"].as_bool().unwrap_or(false),
            is_member,
            requested: res["requested"].as_bool().unwrap_or(false),
            banned: res["banned"].as_bool().unwrap_or(false),
            conversation_id,
        })
    }

    /// Joins with a link (or asks the admins, when the link requires approval).
    pub async fn join_group_by_link(&self, link: &str) -> Result<JoinResult> {
        let secret = parse_invite_link(link)?;
        let (_, blob, token) = self.lookup_invite(&secret).await?;
        let (_, blob_key) = crypto::invite_keys(&secret);
        let s = self.session()?;
        let intro = Intro { name: self.get_profile()?.name, profile_key: self.profile_key_b64()? };
        let intro = crypto::aead_seal(&blob_key, &serde_json::to_vec(&intro)?, INTRO_AAD)?;
        let res: serde_json::Value =
            s.api.post("/v1/invites/join", &json!({ "token": b64(token), "intro": b64(intro) })).await?;
        let status = res["status"].as_str().unwrap_or_default().to_string();
        let gid = blob.group_id.clone();
        let mut conversation_id = None;
        match status.as_str() {
            "joined" => {
                if let Some(k) = &blob.group_key {
                    self.store_group_key(&gid, &unb64(k)?)?;
                }
                // The blob's name until the metadata can be read.
                self.ensure_group_row(&gid)?;
                self.db.borrow().conn.execute(
                    "UPDATE groups SET name = ?2, description = ?3 WHERE group_id = ?1 AND name = ''",
                    params![gid, blob.name, blob.description],
                )?;
                // We asked to join: not a pending invite.
                self.mark_accepted(&self.create_group_conversation(&gid)?)?;
                self.sync_group(&gid, None).await?;
                // Until an admin sends us the attested list, start from the server's (TOFU).
                if self.attested(&gid).is_none() {
                    self.attest_from_server(&gid)?;
                }
                let conv = self.create_group_conversation(&gid)?;
                if blob.group_key.is_some() {
                    let proof = self.join_proof_for(&gid, &secret)?;
                    let id = self.broadcast_group_control_with(&gid, GroupChange::JoinedByLink, &[], Some(proof))?;
                    self.insert_system_message(&conv, "group_member_joined_link", Some(self.me()?), vec![], None, Some(format!("grp-{id}")), now_ms())?;
                    self.share_profile_key_in_group(&gid, None)?;
                }
                self.db.borrow().kv_delete(&format!("join_requested:{gid}"))?;
                conversation_id = Some(conv);
            }
            "requested" => {
                self.db.borrow().kv_set_json(&format!("join_requested:{gid}"), &now_ms())?;
            }
            _ => {
                self.sync_group(&gid, None).await?;
                conversation_id = self.conversation_id_for_group(&gid)?;
                if let Some(conv) = &conversation_id {
                    self.mark_accepted(conv)?;
                }
            }
        }
        Ok(JoinResult { status, conversation_id, group_id: gid })
    }

    /// Withdraws our request to join.
    pub async fn cancel_join_request(&self, group_id: &str) -> Result<()> {
        let me = self.me()?;
        match self.session()?.api.delete::<()>(&format!("/v1/groups/{group_id}/requests/{me}"), None).await {
            Ok(()) => {}
            Err(e) if e.code == ErrorCode::NotFound => {}
            Err(e) => return Err(e),
        }
        self.db.borrow().kv_delete(&format!("join_requested:{group_id}"))?;
        Ok(())
    }

    /// Pending requests to join (admins), with the names the requesters gave.
    pub async fn group_join_requests(self: &Rc<Self>, conv_id: &str) -> Result<Vec<JoinRequest>> {
        let gid = self.require_group_conv(conv_id)?;
        self.require_admin(&gid)?;
        let g = self.group_row(&gid)?.ok_or_else(|| CoreError::not_found("group"))?;
        let blob_key = g.invite_secret.as_deref().and_then(|s| decode_secret(s).ok()).map(|s| crypto::invite_keys(&s).1);
        #[derive(Deserialize)]
        struct Req {
            account_id: String,
            #[serde(default)]
            intro: Option<String>,
            requested_at: i64,
        }
        #[derive(Deserialize)]
        struct Resp {
            requests: Vec<Req>,
        }
        let resp: Resp = self.session()?.api.get(&format!("/v1/groups/{gid}/requests")).await?;
        let mut out = Vec::new();
        for r in resp.requests {
            // The intro opens with the current link's key (requests made with an older link
            // show the name we know, if any).
            let intro: Option<Intro> = match (&blob_key, &r.intro) {
                (Some(k), Some(i)) => unb64(i)
                    .ok()
                    .and_then(|i| crypto::aead_open(k, &i, INTRO_AAD).ok())
                    .and_then(|p| serde_json::from_slice(&p).ok()),
                _ => None,
            };
            let _ = self.ensure_contact(&r.account_id).await;
            if let Some(i) = &intro {
                self.update_profile_key(&r.account_id, &i.profile_key).await;
            }
            let c = self.contact_model(&r.account_id)?;
            let name = intro
                .map(|i| crate::sanitize::clean_display_text(&i.name, 100))
                .filter(|n| !n.is_empty())
                .unwrap_or(c.display_name);
            out.push(JoinRequest { account_id: r.account_id, name, avatar_path: c.avatar_path, requested_at: r.requested_at });
        }
        Ok(out)
    }

    /// Lets a requester in: they get the group key pairwise like any added member.
    pub async fn approve_join_request(&self, conv_id: &str, account_id: &str) -> Result<GroupInfo> {
        let gid = self.require_group_conv(conv_id)?;
        self.require_admin(&gid)?;
        let _: serde_json::Value =
            self.session()?.api.post(&format!("/v1/groups/{gid}/requests/{account_id}/approve"), &json!({})).await?;
        self.sync_group(&gid, None).await?;
        let new = vec![account_id.to_string()];
        self.attest_added(&gid, &new)?;
        let id = self.broadcast_group_control(&gid, GroupChange::MembersAdded { accounts: new.clone() }, &[])?;
        self.share_profile_key_in_group(&gid, Some(new.clone()))?;
        self.insert_system_message(conv_id, "group_member_added", Some(self.me()?), new, None, Some(format!("grp-{id}")), now_ms())?;
        self.group_info(conv_id)
    }

    pub async fn reject_join_request(&self, conv_id: &str, account_id: &str) -> Result<GroupInfo> {
        let gid = self.require_group_conv(conv_id)?;
        self.require_admin(&gid)?;
        self.session()?.api.delete::<()>(&format!("/v1/groups/{gid}/requests/{account_id}"), None).await?;
        self.sync_group(&gid, None).await?;
        self.group_info(conv_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_round_trip() {
        let s = new_invite_secret();
        let link = invite_link(&s);
        assert!(link.starts_with("chatapp://join/"));
        let bytes = parse_invite_link(&link).unwrap();
        assert_eq!(URL_SAFE_NO_PAD.encode(bytes), s);
        // Inside other text, or bare.
        assert_eq!(parse_invite_link(&format!("Join us: {link} !")).unwrap(), bytes);
        assert_eq!(parse_invite_link(&s).unwrap(), bytes);
        assert!(parse_invite_link("chatapp://join/abc").is_err());
        assert!(parse_invite_link("hello").is_err());
        // Token and blob key differ and are stable.
        let (t1, k1) = crypto::invite_keys(&bytes);
        let (t2, k2) = crypto::invite_keys(&bytes);
        assert_eq!((t1, k1), (t2, k2));
        assert_ne!(t1, k1);
    }
}
