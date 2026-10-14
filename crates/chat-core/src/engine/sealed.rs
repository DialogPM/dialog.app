//! Sealed sender: 1:1 messages travel without telling the server who sent them.
//!
//! The sender's identity is inside the end-to-end encrypted envelope (libsignal sealed sender
//! v1), together with a short-lived certificate the server signed for the sender's device.
//! The recipient checks that certificate against the server's trust root. Delivery is not
//! authenticated: the sender proves it may write to the recipient with the recipient's
//! "unidentified access key", derived from the recipient's profile key, which only the
//! recipient's contacts know. Without it (a first message to a stranger), messages go the
//! normal, identified way.
//!
//! Not covered: messages to our own devices (the server knows the account anyway) and group
//! messages (the server checks group membership on delivery).

use std::time::SystemTime;

use libsignal_protocol::{
    sealed_sender_decrypt_to_usmc, sealed_sender_encrypt, DeviceId, ProtocolAddress, PublicKey, SenderCertificate,
    SessionStore, Timestamp,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::Inner;
use crate::api::{Envelope, OutgoingMessageDto};
use crate::crypto::{self, b64, unb64};
use crate::error::{CoreError, Result};
use crate::util::now_ms;

/// Envelope type of sealed messages.
pub const SEALED_ENVELOPE: u8 = 6;
/// A cached certificate is replaced when it has less than this left.
const CERT_REFRESH_MARGIN_MS: i64 = 2 * 3600 * 1000;

#[derive(Serialize, Deserialize)]
struct CachedCertificate {
    certificate: String,
    expires: i64,
}

impl Inner {
    /// Fetches the trust root and publishes our access key (each time the connection starts).
    pub(super) async fn sealed_setup(&self) -> Result<()> {
        let s = self.session()?;
        let info = s.api.info().await?;
        self.db.borrow().kv_set_json("max_attachment_bytes", &info.max_attachment_bytes)?;
        if let Some(root) = info.sealed_sender_trust_root {
            let key = unb64(&root)?;
            PublicKey::deserialize(&key)?;
            self.db.borrow().kv_set("sealed_trust_root", &key)?;
        }
        let access_key = b64(crypto::unidentified_access_key(&self.profile_key()?));
        let uploaded: Option<String> = self.db.borrow().kv_get_json("unidentified_access_uploaded")?;
        if uploaded.as_deref() != Some(access_key.as_str()) {
            let _: serde_json::Value = s.api.put("/v1/account/unidentified-access", &json!({ "key": access_key })).await?;
            self.db.borrow().kv_set_json("unidentified_access_uploaded", &access_key)?;
        }
        Ok(())
    }

    fn trust_root(&self) -> Option<PublicKey> {
        let raw = self.db.borrow().kv_get("sealed_trust_root").ok().flatten()?;
        PublicKey::deserialize(&raw).ok()
    }

    /// Our sender certificate (cached, renewed before it expires). None: send identified.
    pub(super) async fn sender_certificate(&self) -> Option<SenderCertificate> {
        let cached: Option<CachedCertificate> = self.db.borrow().kv_get_json("sender_certificate").ok().flatten();
        if let Some(c) = cached.filter(|c| c.expires - now_ms() > CERT_REFRESH_MARGIN_MS) {
            if let Some(cert) = unb64(&c.certificate).ok().and_then(|b| SenderCertificate::deserialize(&b).ok()) {
                return Some(cert);
            }
        }
        let fetched: Result<CachedCertificate> = async { self.session()?.api.get("/v1/certificate/delivery").await }.await;
        match fetched {
            Ok(c) => {
                let cert = SenderCertificate::deserialize(&unb64(&c.certificate).ok()?).ok()?;
                // Never use a certificate that does not verify against the trust root.
                let root = self.trust_root()?;
                if !cert.validate(&root, Timestamp::from_epoch_millis(now_ms() as u64)).unwrap_or(false) {
                    log::error!("the server issued a sender certificate that does not verify");
                    return None;
                }
                let _ = self.db.borrow().kv_set_json("sender_certificate", &c);
                Some(cert)
            }
            Err(e) => {
                log::warn!("no sender certificate ({e}); sending identified");
                None
            }
        }
    }

    /// The access key of a contact, known once we have their profile key.
    pub(super) fn peer_access_key(&self, account_id: &str) -> Option<[u8; 16]> {
        let key: Option<Vec<u8>> = self
            .db
            .borrow()
            .conn
            .query_row("SELECT profile_key FROM contacts WHERE account_id = ?1", [account_id], |r| r.get(0))
            .ok()
            .flatten();
        let key = crypto::key32(&key?).ok()?;
        Some(crypto::unidentified_access_key(&key))
    }

    /// Like encrypt_for, but every device message is sealed (type 6).
    pub(super) async fn seal_for(
        &self,
        account_id: &str,
        devices: &[u32],
        plaintext: &[u8],
        cert: &SenderCertificate,
    ) -> Result<Vec<OutgoingMessageDto>> {
        let mut out = Vec::with_capacity(devices.len());
        for &d in devices {
            let address = ProtocolAddress::new(
                account_id.to_string(),
                DeviceId::new(d as u8).map_err(|_| CoreError::internal("invalid device id"))?,
            );
            let mut sessions = self.signal_store()?;
            let mut identities = self.signal_store()?;
            let sealed = sealed_sender_encrypt(
                &address,
                cert,
                plaintext,
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
            out.push(OutgoingMessageDto { device_id: d, registration_id, kind: SEALED_ENVELOPE, content: b64(sealed) });
        }
        Ok(out)
    }

    /// Opens the outer layer of a sealed envelope: checks the sender certificate and turns the
    /// envelope into the ordinary one it wraps (sender filled in, inner message returned).
    pub(super) async fn unseal(&self, env: &mut Envelope, bytes: &[u8]) -> Result<Vec<u8>> {
        if self.trust_root().is_none() {
            self.sealed_setup().await?;
        }
        let root = self.trust_root().ok_or_else(|| CoreError::internal("no sealed sender trust root"))?;
        let store = self.signal_store()?;
        let usmc = sealed_sender_decrypt_to_usmc(bytes, &store).await?;
        let sender = usmc.sender()?;
        let at = if env.server_timestamp > 0 { env.server_timestamp } else { now_ms() };
        if !sender.validate(&root, Timestamp::from_epoch_millis(at as u64))? {
            return Err(CoreError::internal("sealed message with an invalid sender certificate"));
        }
        let s = self.session()?;
        let device: u32 = sender.sender_device_id()?.into();
        if sender.sender_uuid()? == s.account.account_id && device == s.account.device_id {
            return Err(CoreError::internal("sealed message from ourselves"));
        }
        env.source_account = sender.sender_uuid()?.to_string();
        env.source_device = device;
        env.kind = usmc.msg_type()? as u8;
        Ok(usmc.contents()?.to_vec())
    }
}
