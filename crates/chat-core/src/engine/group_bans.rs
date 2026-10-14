//! Group bans (admins): accounts an admin removed, or whose join request an admin rejected,
//! can't rejoin with the invite link. Admins list them and lift a ban; adding the account
//! back lifts it too (server side).

use serde::{Deserialize, Serialize};

use super::Inner;
use crate::error::Result;
use crate::model::GroupInfo;

#[derive(Debug, Clone, Serialize)]
pub struct GroupBan {
    pub account_id: String,
    pub display_name: String,
    pub avatar_path: Option<String>,
    /// Who banned them (an admin's account id).
    pub banned_by: String,
    pub banned_at: i64,
}

impl Inner {
    /// Accounts that can't rejoin the group with the link (admins), newest first.
    pub async fn group_bans(&self, conv_id: &str) -> Result<Vec<GroupBan>> {
        let gid = self.require_group_conv(conv_id)?;
        self.require_admin(&gid)?;
        #[derive(Deserialize)]
        struct Ban {
            account_id: String,
            #[serde(default)]
            banned_by: String,
            #[serde(default)]
            banned_at: i64,
        }
        #[derive(Deserialize)]
        struct Resp {
            bans: Vec<Ban>,
        }
        let resp: Resp = self.session()?.api.get(&format!("/v1/groups/{gid}/bans")).await?;
        let mut out = Vec::with_capacity(resp.bans.len());
        for b in resp.bans {
            let _ = self.ensure_contact(&b.account_id).await;
            let c = self.contact_model(&b.account_id)?;
            out.push(GroupBan {
                account_id: b.account_id,
                display_name: c.display_name,
                avatar_path: c.avatar_path,
                banned_by: b.banned_by,
                banned_at: b.banned_at,
            });
        }
        Ok(out)
    }

    /// Lets a banned account rejoin with the link (or ask to) again (admins).
    pub async fn unban_group_member(&self, conv_id: &str, account_id: &str) -> Result<GroupInfo> {
        let gid = self.require_group_conv(conv_id)?;
        self.require_admin(&gid)?;
        self.session()?.api.delete::<()>(&format!("/v1/groups/{gid}/bans/{account_id}"), None).await?;
        self.sync_group(&gid, None).await?;
        self.group_info(conv_id)
    }
}
