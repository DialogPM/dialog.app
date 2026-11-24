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


use libsignal_protocol::{
    sealed_sender_decrypt_to_usmc, sealed_sender_encrypt, CiphertextMessageType, DeviceId, ProtocolAddress, PublicKey,
    SenderCertificate, SessionStore, Timestamp,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::Inner;
use crate::api::{Envelope, OutgoingMessageDto};
use crate::crypto::{self, b64, unb64};
use crate::error::{CoreError, ErrorCode, Result};
use crate::util::now_ms;

/// Envelope type of sealed messages.
pub const SEALED_ENVELOPE: u8 = 6;
/// A cached certificate is replaced when it has less than this left.
const CERT_REFRESH_MARGIN_MS: i64 = 2 * 3600 * 1000;
/// How long a message may wait on the server before we receive it (the server's default
/// message lifetime). The server's receive timestamp tells when the sender's certificate had
/// to be valid, but the server chooses it: it may not move that moment further back than this.
const MAX_QUEUE_MS: i64 = 30 * 24 * 3600 * 1000;
const TRUST_ROOT_KEY: &str = "sealed_trust_root";

/// The moment a sealed message's sender certificate must still be valid at: when the server
/// received the message, but never earlier than the longest a message waits for us.
fn certificate_validation_time(server_timestamp: i64, now: i64) -> i64 {
    if server_timestamp > 0 {
        server_timestamp.max(now - MAX_QUEUE_MS)
    } else {
        now
    }
}

/// Only 1:1 messages are sealed (engine/send.rs): a sealed envelope wraps a Signal message or
/// a pre-key message, never a group (sender key) message or anything else.
fn sealed_inner_kind(t: CiphertextMessageType) -> Option<u8> {
    match t {
        CiphertextMessageType::Whisper | CiphertextMessageType::PreKey => Some(t as u8),
        _ => None,
    }
}

/// Account ids are UUIDs: the same account whatever the letter case.
pub(super) fn same_account(a: &str, b: &str) -> bool {
    match (uuid::Uuid::parse_str(a), uuid::Uuid::parse_str(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a.eq_ignore_ascii_case(b),
    }
}

#[derive(Serialize, Deserialize)]
struct CachedCertificate {
    certificate: String,
    expires: i64,
}

impl Inner {
    /// Pins the trust root and publishes our access key (each time the connection starts).
    /// Like the zkgroup parameters and the key transparency key, the trust root is pinned on
    /// first contact and a changed one is refused: a server that swaps it could vouch for
    /// certificates it never issued before.
    pub(super) async fn sealed_setup(&self) -> Result<()> {
        let s = self.session()?;
        let info = s.api.info().await?;
        self.db.borrow().kv_set_json("max_attachment_bytes", &info.max_attachment_bytes)?;
        if let Some(root) = info.sealed_sender_trust_root {
            let key = unb64(&root)?;
            PublicKey::deserialize(&key)?;
            let pinned = self.db.borrow().kv_get(TRUST_ROOT_KEY)?;
            match pinned {
                None => self.db.borrow().kv_set(TRUST_ROOT_KEY, &key)?,
                Some(p) if p == key => {}
                Some(_) => {
                    log::error!("the server's sealed sender trust root changed: refusing it");
                    return Err(CoreError::new(ErrorCode::Forbidden, "the server's sealed sender key changed"));
                }
            }
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
        let raw = self.db.borrow().kv_get(TRUST_ROOT_KEY).ok().flatten()?;
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
                self.session_now(),
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
        let at = certificate_validation_time(env.server_timestamp, now_ms());
        if !sender.validate(&root, Timestamp::from_epoch_millis(at.max(0) as u64))? {
            return Err(CoreError::internal("sealed message with an invalid sender certificate"));
        }
        let kind = sealed_inner_kind(usmc.msg_type()?)
            .ok_or_else(|| CoreError::internal("sealed message of a type that is never sealed"))?;
        let s = self.session()?;
        // Our own devices never seal for each other.
        if same_account(sender.sender_uuid()?, &s.account.account_id) {
            return Err(CoreError::internal("sealed message from our own account"));
        }
        env.source_account = sender.sender_uuid()?.to_ascii_lowercase();
        env.source_device = sender.sender_device_id()?.into();
        env.kind = kind;
        Ok(usmc.contents()?.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use libsignal_protocol::{IdentityKeyPair, KeyPair, ServerCertificate};

    const DAY: i64 = 24 * 3600 * 1000;

    /// A trust root and a sender certificate it vouches for, expiring at `expires`.
    fn certificate(expires: i64) -> (PublicKey, SenderCertificate) {
        let mut rng = rand::rng();
        let root = KeyPair::generate(&mut rng);
        let server = KeyPair::generate(&mut rng);
        let server_cert = ServerCertificate::new(1, server.public_key, &root.private_key, &mut rng).unwrap();
        let identity = IdentityKeyPair::generate(&mut rng);
        let cert = SenderCertificate::new(
            uuid::Uuid::new_v4().to_string(),
            None,
            *identity.public_key(),
            DeviceId::new(1).unwrap(),
            Timestamp::from_epoch_millis(expires as u64),
            server_cert,
            &server.private_key,
            &mut rng,
        )
        .unwrap();
        (root.public_key, cert)
    }

    fn valid(root: &PublicKey, cert: &SenderCertificate, server_timestamp: i64, now: i64) -> bool {
        let at = certificate_validation_time(server_timestamp, now);
        cert.validate(root, Timestamp::from_epoch_millis(at as u64)).unwrap()
    }

    #[test]
    fn the_server_cannot_revive_an_expired_certificate() {
        let now = 1_800_000_000_000;
        // Expired 40 days ago: a receive time the server sets far in the past does not help.
        let (root, cert) = certificate(now - 40 * DAY);
        assert!(!valid(&root, &cert, 1, now));
        assert!(!valid(&root, &cert, now - 41 * DAY, now));
        assert!(!valid(&root, &cert, now, now));
    }

    #[test]
    fn a_message_that_waited_on_the_server_is_accepted() {
        let now = 1_800_000_000_000;
        // Sent 20 days ago with a certificate valid for one more day back then.
        let sent = now - 20 * DAY;
        let (root, cert) = certificate(sent + DAY);
        assert!(valid(&root, &cert, sent, now));
        // A fresh one, and one whose envelope has no receive time.
        let (root, cert) = certificate(now + DAY);
        assert!(valid(&root, &cert, now - 1000, now));
        assert!(valid(&root, &cert, 0, now));
        // Waited longer than the server keeps messages (with a lifetime set above the default):
        // refused.
        let sent = now - 40 * DAY;
        let (root, cert) = certificate(sent + DAY);
        assert!(!valid(&root, &cert, sent, now));
    }

    #[test]
    fn only_one_to_one_messages_are_unsealed() {
        assert_eq!(sealed_inner_kind(CiphertextMessageType::Whisper), Some(2));
        assert_eq!(sealed_inner_kind(CiphertextMessageType::PreKey), Some(3));
        assert_eq!(sealed_inner_kind(CiphertextMessageType::SenderKey), None);
        assert_eq!(sealed_inner_kind(CiphertextMessageType::Plaintext), None);
    }

    #[test]
    fn account_ids_compare_whatever_the_case() {
        let id = "0b7c5f3e-8d1a-4c2b-9e6f-1a2b3c4d5e6f";
        assert!(same_account(id, &id.to_ascii_uppercase()));
        assert!(same_account(&id.to_ascii_uppercase(), id));
        assert!(!same_account(id, "0b7c5f3e-8d1a-4c2b-9e6f-1a2b3c4d5e60"));
    }
}
