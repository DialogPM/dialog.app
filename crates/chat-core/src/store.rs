//! libsignal storage traits implemented on top of the local database.

use std::cell::RefCell;
use std::rc::Rc;

use async_trait::async_trait;
use libsignal_protocol::{
    Direction, GenericSignedPreKey, IdentityChange, IdentityKey, IdentityKeyPair, IdentityKeyStore,
    KyberPreKeyId, KyberPreKeyRecord, KyberPreKeyStore, PreKeyId, PreKeyRecord, PreKeyStore,
    ProtocolAddress, PublicKey, SenderKeyRecord, SenderKeyStore, SessionRecord, SessionStore,
    SignalProtocolError, SignedPreKeyId, SignedPreKeyRecord, SignedPreKeyStore,
};
use rusqlite::{params, OptionalExtension};
use uuid::Uuid;

use crate::db::Db;
use crate::util::now_ms;

type SResult<T> = std::result::Result<T, SignalProtocolError>;

fn db_err(e: impl std::fmt::Display) -> SignalProtocolError {
    SignalProtocolError::InvalidState("database", e.to_string())
}

/// Signal protocol store. Cheap to construct; all state lives in the database.
///
/// Identity keys are pinned: an account's identity key is derived from its recovery phrase
/// and never changes (not on password change, not on recovery), and every device of the
/// account uses it. So a device presenting a different key is not "a new phone" but a device
/// the account owner never created (e.g. injected by a compromised server). Such devices are
/// refused instead of trusted-on-change like Signal's default.
pub struct SignalStore {
    pub db: Rc<RefCell<Db>>,
    pub identity: IdentityKeyPair,
    pub registration_id: u32,
    /// Our account id: its devices must present our own identity key.
    pub account_id: String,
    /// Accounts for which a device with a mismatching identity key was refused during the
    /// last operations (drained by the engine, which warns the user).
    pub identity_conflicts: Rc<RefCell<Vec<String>>>,
}

impl SignalStore {
    fn pinned_identity(&self, account: &str) -> SResult<Option<Vec<u8>>> {
        if account == self.account_id {
            return Ok(Some(self.identity.identity_key().serialize().to_vec()));
        }
        self.db
            .borrow()
            .conn
            .query_row("SELECT identity_key FROM identities WHERE account_id = ?1", [account], |r| r.get(0))
            .optional()
            .map_err(db_err)
    }
}

fn device_of(address: &ProtocolAddress) -> i64 {
    u32::from(address.device_id()) as i64
}

#[async_trait(?Send)]
impl IdentityKeyStore for SignalStore {
    async fn get_identity_key_pair(&self) -> SResult<IdentityKeyPair> {
        Ok(self.identity)
    }

    async fn get_local_registration_id(&self) -> SResult<u32> {
        Ok(self.registration_id)
    }

    async fn save_identity(
        &mut self,
        address: &ProtocolAddress,
        identity: &IdentityKey,
    ) -> SResult<IdentityChange> {
        let existing = self.pinned_identity(address.name())?;
        let db = self.db.borrow();
        let new = identity.serialize();
        match existing {
            Some(old) if old[..] == new[..] => Ok(IdentityChange::NewOrUnchanged),
            Some(_) => {
                // Never replace a pinned identity (is_trusted_identity refuses it first; this
                // is a second line of defence).
                self.identity_conflicts.borrow_mut().push(address.name().to_string());
                Err(SignalProtocolError::UntrustedIdentity(address.clone()))
            }
            None => {
                db.conn
                    .execute(
                        "INSERT INTO identities (account_id, identity_key, first_seen) VALUES (?1, ?2, ?3)",
                        params![address.name(), &new[..], now_ms()],
                    )
                    .map_err(db_err)?;
                // Share the pin with our other devices (vault contact record), when the contact exists.
                db.conn
                    .execute(
                        "INSERT OR IGNORE INTO vault_dirty (logical_id) SELECT 'contact:' || account_id FROM contacts WHERE account_id = ?1",
                        [address.name()],
                    )
                    .map_err(db_err)?;
                Ok(IdentityChange::NewOrUnchanged)
            }
        }
    }

    async fn is_trusted_identity(
        &self,
        address: &ProtocolAddress,
        identity: &IdentityKey,
        _direction: Direction,
    ) -> SResult<bool> {
        // Trust on first use, then pinned (see the type's documentation).
        match self.pinned_identity(address.name())? {
            Some(pinned) if pinned[..] != identity.serialize()[..] => {
                log::error!("refusing {address}: identity key differs from the account's pinned key");
                self.identity_conflicts.borrow_mut().push(address.name().to_string());
                Ok(false)
            }
            _ => Ok(true),
        }
    }

    async fn get_identity(&self, address: &ProtocolAddress) -> SResult<Option<IdentityKey>> {
        self.pinned_identity(address.name())?.map(|b| IdentityKey::decode(&b)).transpose()
    }
}

#[async_trait(?Send)]
impl PreKeyStore for SignalStore {
    async fn get_pre_key(&self, id: PreKeyId) -> SResult<PreKeyRecord> {
        let db = self.db.borrow();
        let v: Option<Vec<u8>> = db
            .conn
            .query_row("SELECT record FROM prekeys WHERE id = ?1", [u32::from(id)], |r| r.get(0))
            .optional()
            .map_err(db_err)?;
        match v {
            Some(b) => PreKeyRecord::deserialize(&b),
            None => Err(SignalProtocolError::InvalidPreKeyId),
        }
    }

    async fn save_pre_key(&mut self, id: PreKeyId, record: &PreKeyRecord) -> SResult<()> {
        self.db
            .borrow()
            .conn
            .execute(
                "INSERT OR REPLACE INTO prekeys (id, record) VALUES (?1, ?2)",
                params![u32::from(id), record.serialize()?],
            )
            .map_err(db_err)?;
        Ok(())
    }

    async fn remove_pre_key(&mut self, id: PreKeyId) -> SResult<()> {
        self.db
            .borrow()
            .conn
            .execute("DELETE FROM prekeys WHERE id = ?1", [u32::from(id)])
            .map_err(db_err)?;
        Ok(())
    }
}

#[async_trait(?Send)]
impl SignedPreKeyStore for SignalStore {
    async fn get_signed_pre_key(&self, id: SignedPreKeyId) -> SResult<SignedPreKeyRecord> {
        let db = self.db.borrow();
        let v: Option<Vec<u8>> = db
            .conn
            .query_row(
                "SELECT record FROM signed_prekeys WHERE id = ?1",
                [u32::from(id)],
                |r| r.get(0),
            )
            .optional()
            .map_err(db_err)?;
        match v {
            Some(b) => SignedPreKeyRecord::deserialize(&b),
            None => Err(SignalProtocolError::InvalidSignedPreKeyId),
        }
    }

    async fn save_signed_pre_key(
        &mut self,
        id: SignedPreKeyId,
        record: &SignedPreKeyRecord,
    ) -> SResult<()> {
        self.db
            .borrow()
            .conn
            .execute(
                "INSERT OR REPLACE INTO signed_prekeys (id, record, created_at) VALUES (?1, ?2, ?3)",
                params![u32::from(id), record.serialize()?, now_ms()],
            )
            .map_err(db_err)?;
        Ok(())
    }
}

#[async_trait(?Send)]
impl KyberPreKeyStore for SignalStore {
    async fn get_kyber_pre_key(&self, id: KyberPreKeyId) -> SResult<KyberPreKeyRecord> {
        let db = self.db.borrow();
        let v: Option<Vec<u8>> = db
            .conn
            .query_row(
                "SELECT record FROM kyber_prekeys WHERE id = ?1",
                [u32::from(id)],
                |r| r.get(0),
            )
            .optional()
            .map_err(db_err)?;
        match v {
            Some(b) => KyberPreKeyRecord::deserialize(&b),
            None => Err(SignalProtocolError::InvalidKyberPreKeyId),
        }
    }

    async fn save_kyber_pre_key(
        &mut self,
        id: KyberPreKeyId,
        record: &KyberPreKeyRecord,
    ) -> SResult<()> {
        self.db
            .borrow()
            .conn
            .execute(
                "INSERT OR REPLACE INTO kyber_prekeys (id, record, last_resort, created_at) VALUES (?1, ?2, 0, ?3)",
                params![u32::from(id), record.serialize()?, now_ms()],
            )
            .map_err(db_err)?;
        Ok(())
    }

    async fn mark_kyber_pre_key_used(
        &mut self,
        id: KyberPreKeyId,
        ec_prekey_id: SignedPreKeyId,
        base_key: &PublicKey,
    ) -> SResult<()> {
        let db = self.db.borrow();
        let last_resort: Option<i64> = db
            .conn
            .query_row(
                "SELECT last_resort FROM kyber_prekeys WHERE id = ?1",
                [u32::from(id)],
                |r| r.get(0),
            )
            .optional()
            .map_err(db_err)?;
        match last_resort {
            Some(1) => {
                // Last-resort keys are reused; reject replays of the same key combination.
                let inserted = db
                    .conn
                    .execute(
                        "INSERT OR IGNORE INTO kyber_used (kyber_id, signed_id, base_key) VALUES (?1, ?2, ?3)",
                        params![u32::from(id), u32::from(ec_prekey_id), &base_key.serialize()[..]],
                    )
                    .map_err(db_err)?;
                if inserted == 0 {
                    return Err(SignalProtocolError::InvalidMessage(
                        libsignal_protocol::CiphertextMessageType::PreKey,
                        "reused base key".to_string(),
                    ));
                }
            }
            Some(_) => {
                db.conn
                    .execute("DELETE FROM kyber_prekeys WHERE id = ?1", [u32::from(id)])
                    .map_err(db_err)?;
            }
            None => {}
        }
        Ok(())
    }
}

#[async_trait(?Send)]
impl SessionStore for SignalStore {
    async fn load_session(&self, address: &ProtocolAddress) -> SResult<Option<SessionRecord>> {
        let db = self.db.borrow();
        let v: Option<Vec<u8>> = db
            .conn
            .query_row(
                "SELECT record FROM sessions WHERE account_id = ?1 AND device_id = ?2",
                params![address.name(), device_of(address)],
                |r| r.get(0),
            )
            .optional()
            .map_err(db_err)?;
        v.map(|b| SessionRecord::deserialize(&b)).transpose()
    }

    async fn store_session(
        &mut self,
        address: &ProtocolAddress,
        record: &SessionRecord,
    ) -> SResult<()> {
        self.db
            .borrow()
            .conn
            .execute(
                "INSERT OR REPLACE INTO sessions (account_id, device_id, record) VALUES (?1, ?2, ?3)",
                params![address.name(), device_of(address), record.serialize()?],
            )
            .map_err(db_err)?;
        Ok(())
    }
}

#[async_trait(?Send)]
impl SenderKeyStore for SignalStore {
    async fn store_sender_key(
        &mut self,
        sender: &ProtocolAddress,
        distribution_id: Uuid,
        record: &SenderKeyRecord,
    ) -> SResult<()> {
        self.db
            .borrow()
            .conn
            .execute(
                "INSERT OR REPLACE INTO sender_keys (account_id, device_id, distribution_id, record) VALUES (?1, ?2, ?3, ?4)",
                params![sender.name(), device_of(sender), distribution_id.to_string(), record.serialize()?],
            )
            .map_err(db_err)?;
        Ok(())
    }

    async fn load_sender_key(
        &mut self,
        sender: &ProtocolAddress,
        distribution_id: Uuid,
    ) -> SResult<Option<SenderKeyRecord>> {
        let db = self.db.borrow();
        let v: Option<Vec<u8>> = db
            .conn
            .query_row(
                "SELECT record FROM sender_keys WHERE account_id = ?1 AND device_id = ?2 AND distribution_id = ?3",
                params![sender.name(), device_of(sender), distribution_id.to_string()],
                |r| r.get(0),
            )
            .optional()
            .map_err(db_err)?;
        v.map(|b| SenderKeyRecord::deserialize(&b)).transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use libsignal_protocol::DeviceId;

    fn store(account_id: &str) -> SignalStore {
        SignalStore {
            db: Rc::new(RefCell::new(Db::open_in_memory().unwrap())),
            identity: IdentityKeyPair::generate(&mut rand::rng()),
            registration_id: 1,
            account_id: account_id.into(),
            identity_conflicts: Rc::new(RefCell::new(Vec::new())),
        }
    }

    fn addr(account: &str, device: u8) -> ProtocolAddress {
        ProtocolAddress::new(account.into(), DeviceId::new(device).unwrap())
    }

    #[tokio::test]
    async fn identities_are_pinned() {
        let mut st = store("me");
        let bob = *IdentityKeyPair::generate(&mut rand::rng()).identity_key();
        let evil = *IdentityKeyPair::generate(&mut rand::rng()).identity_key();
        // First contact: trusted and pinned, for every device of the account.
        assert!(st.is_trusted_identity(&addr("bob", 1), &bob, Direction::Sending).await.unwrap());
        st.save_identity(&addr("bob", 1), &bob).await.unwrap();
        assert!(st.is_trusted_identity(&addr("bob", 2), &bob, Direction::Receiving).await.unwrap());
        // Another key for the same account is refused, recorded, and never stored.
        assert!(!st.is_trusted_identity(&addr("bob", 3), &evil, Direction::Sending).await.unwrap());
        assert!(st.save_identity(&addr("bob", 3), &evil).await.is_err());
        assert_eq!(st.get_identity(&addr("bob", 1)).await.unwrap(), Some(bob));
        assert_eq!(st.identity_conflicts.borrow().as_slice(), ["bob", "bob"]);
    }

    #[tokio::test]
    async fn own_devices_must_use_our_identity() {
        let st = store("me");
        let mine = *st.identity.identity_key();
        let evil = *IdentityKeyPair::generate(&mut rand::rng()).identity_key();
        assert!(st.is_trusted_identity(&addr("me", 2), &mine, Direction::Sending).await.unwrap());
        assert!(!st.is_trusted_identity(&addr("me", 3), &evil, Direction::Sending).await.unwrap());
    }
}
