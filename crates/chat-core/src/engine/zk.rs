//! zkgroup auth credentials (docs/ZKGROUP-DESIGN.md, step 1): the server proves nothing about
//! who we are when we present them for a group, yet can check we are a member.
//!
//! The server's public parameters are pinned on first contact; a change is refused (it would let
//! a server swap keys under us) until the user resets the account on that server.

use std::collections::BTreeMap;

use serde::Deserialize;
use zkgroup::auth::{AuthCredentialWithPniZkc, AuthCredentialWithPniZkcResponse};
use zkgroup::groups::GroupSecretParams;
use zkgroup::{ServerPublicParams, Timestamp, SECONDS_PER_DAY};

use super::Inner;
use crate::crypto::{b64, unb64};
use crate::error::{CoreError, ErrorCode, Result};
use crate::util::now_ms;

/// Same salt as the server: accounts have no phone-number identity ("false PNI").
pub const PNI_SALT: &[u8] = b"chatapp/zkgroup/no-pni";
const PARAMS_KEY: &str = "zk_public_params";
const CREDS_KEY: &str = "zk_auth_credentials";
const DAYS: u64 = 7;

fn randomness() -> [u8; zkgroup::RANDOMNESS_LEN] {
    crate::crypto::random_bytes::<{ zkgroup::RANDOMNESS_LEN }>()
}

fn today() -> u64 {
    let now = now_ms() as u64 / 1000;
    now - now % SECONDS_PER_DAY
}

impl Inner {
    /// Pins the server's zkgroup parameters (each time the connection starts).
    pub(super) async fn zk_setup(&self) -> Result<()> {
        let info = self.session()?.api.info().await?;
        self.note_groups_v2(&info)?;
        let Some(params) = info.zkgroup_public_params else { return Ok(()) };
        let bytes = unb64(&params)?;
        zkgroup::deserialize::<ServerPublicParams>(&bytes).map_err(|_| CoreError::internal("invalid zkgroup parameters"))?;
        let pinned: Option<String> = self.db.borrow().kv_get_json(PARAMS_KEY)?;
        match pinned {
            None => self.db.borrow().kv_set_json(PARAMS_KEY, &params)?,
            Some(p) if p == params => {}
            Some(_) => {
                log::error!("the server's zkgroup parameters changed: refusing them");
                return Err(CoreError::new(ErrorCode::Forbidden, "the server's group keys changed"));
            }
        }
        Ok(())
    }

    pub(super) fn zk_public_params(&self) -> Result<ServerPublicParams> {
        let p: String = self
            .db
            .borrow()
            .kv_get_json(PARAMS_KEY)?
            .ok_or_else(|| CoreError::new(ErrorCode::NotFound, "this server has no private groups"))?;
        zkgroup::deserialize(&unb64(&p)?).map_err(|_| CoreError::internal("invalid pinned zkgroup parameters"))
    }

    /// Fetches and verifies a week of auth credentials (an invalid one is a server error).
    async fn refresh_auth_credentials(&self) -> Result<BTreeMap<u64, String>> {
        let params = self.zk_public_params()?;
        let s = self.session()?;
        let aci = libsignal_core::Aci::from(
            uuid::Uuid::parse_str(&s.account.account_id).map_err(|e| CoreError::internal(e.to_string()))?,
        );
        #[derive(Deserialize)]
        struct Item {
            redemption_time: u64,
            credential: String,
        }
        #[derive(Deserialize)]
        struct Resp {
            credentials: Vec<Item>,
        }
        let start = today();
        let end = start + (DAYS - 1) * SECONDS_PER_DAY;
        let r: Resp = s
            .api
            .get(&format!("/v1/certificate/auth/group?redemption_start_seconds={start}&redemption_end_seconds={end}"))
            .await?;
        let mut out = BTreeMap::new();
        for item in r.credentials {
            if item.redemption_time < start || item.redemption_time > end || item.redemption_time % SECONDS_PER_DAY != 0 {
                continue;
            }
            let resp: AuthCredentialWithPniZkcResponse = zkgroup::deserialize(&unb64(&item.credential)?)
                .map_err(|_| CoreError::internal("malformed auth credential"))?;
            let cred = resp
                .receive_without_pni(aci, PNI_SALT, Timestamp::from_epoch_seconds(item.redemption_time), &params)
                .map_err(|_| CoreError::internal("the server issued an invalid auth credential"))?;
            out.insert(item.redemption_time, b64(zkgroup::serialize(&cred)));
        }
        self.db.borrow().kv_set_json(CREDS_KEY, &out)?;
        Ok(out)
    }

    /// Today's auth credential (fetched when missing).
    async fn auth_credential(&self) -> Result<AuthCredentialWithPniZkc> {
        let day = today();
        let mut creds: BTreeMap<u64, String> = self.db.borrow().kv_get_json(CREDS_KEY)?.unwrap_or_default();
        if !creds.contains_key(&day) {
            creds = self.refresh_auth_credentials().await?;
        }
        let c = creds.get(&day).ok_or_else(|| CoreError::internal("no auth credential for today"))?;
        zkgroup::deserialize(&unb64(c)?).map_err(|_| CoreError::internal("invalid stored auth credential"))
    }

    /// Proof, for the group with these parameters, that we are the account whose encrypted id
    /// the group lists (the server learns nothing else).
    pub async fn zk_auth_presentation(&self, group: &GroupSecretParams) -> Result<Vec<u8>> {
        let params = self.zk_public_params()?;
        let cred = self.auth_credential().await?;
        Ok(zkgroup::serialize(&cred.present(&params, group, randomness())))
    }
}
