//! Local encrypted database (SQLCipher).

use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};

use crate::error::Result;

const MIGRATIONS: &[&str] = &[r#"
CREATE TABLE kv (
    key   TEXT PRIMARY KEY,
    value BLOB NOT NULL
);

-- Signal protocol state ------------------------------------------------------
CREATE TABLE identities (
    account_id   TEXT PRIMARY KEY,
    identity_key BLOB NOT NULL,
    first_seen   INTEGER NOT NULL
);
CREATE TABLE sessions (
    account_id TEXT NOT NULL,
    device_id  INTEGER NOT NULL,
    record     BLOB NOT NULL,
    PRIMARY KEY (account_id, device_id)
);
CREATE TABLE prekeys (
    id     INTEGER PRIMARY KEY,
    record BLOB NOT NULL
);
CREATE TABLE signed_prekeys (
    id         INTEGER PRIMARY KEY,
    record     BLOB NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE TABLE kyber_prekeys (
    id          INTEGER PRIMARY KEY,
    record      BLOB NOT NULL,
    last_resort INTEGER NOT NULL DEFAULT 0,
    created_at  INTEGER NOT NULL
);
CREATE TABLE kyber_used (
    kyber_id  INTEGER NOT NULL,
    signed_id INTEGER NOT NULL,
    base_key  BLOB NOT NULL,
    PRIMARY KEY (kyber_id, signed_id, base_key)
);
CREATE TABLE sender_keys (
    account_id      TEXT NOT NULL,
    device_id       INTEGER NOT NULL,
    distribution_id TEXT NOT NULL,
    record          BLOB NOT NULL,
    PRIMARY KEY (account_id, device_id, distribution_id)
);
-- Devices that already received our sender key for a distribution id.
CREATE TABLE sender_key_shared (
    distribution_id TEXT NOT NULL,
    account_id      TEXT NOT NULL,
    device_id       INTEGER NOT NULL,
    PRIMARY KEY (distribution_id, account_id, device_id)
);

-- Application data --------------------------------------------------------------
CREATE TABLE contacts (
    account_id       TEXT PRIMARY KEY,
    number           TEXT NOT NULL DEFAULT '',
    profile_key      BLOB,
    profile_name     TEXT,
    about            TEXT,
    avatar_id        TEXT,
    avatar_key       BLOB,
    avatar_digest    BLOB,
    avatar_path      TEXT,
    local_name       TEXT,
    phone            TEXT,
    blocked          INTEGER NOT NULL DEFAULT 0,
    verified         INTEGER NOT NULL DEFAULT 0,
    is_contact       INTEGER NOT NULL DEFAULT 0,
    profile_fetched_at INTEGER NOT NULL DEFAULT 0,
    updated_at       INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX contacts_number ON contacts(number);

CREATE TABLE conversations (
    id                   TEXT PRIMARY KEY,
    kind                 TEXT NOT NULL,
    peer_account_id      TEXT UNIQUE,
    group_id             TEXT UNIQUE,
    muted_until          INTEGER,
    archived             INTEGER NOT NULL DEFAULT 0,
    pinned               INTEGER NOT NULL DEFAULT 0,
    disappearing_seconds INTEGER NOT NULL DEFAULT 0,
    draft                TEXT,
    unread_count         INTEGER NOT NULL DEFAULT 0,
    last_message_id      TEXT,
    hidden               INTEGER NOT NULL DEFAULT 0,
    created_at           INTEGER NOT NULL,
    updated_at           INTEGER NOT NULL
);

CREATE TABLE groups (
    group_id        TEXT PRIMARY KEY,
    group_key       BLOB,
    revision        INTEGER NOT NULL DEFAULT 0,
    name            TEXT NOT NULL DEFAULT '',
    description     TEXT NOT NULL DEFAULT '',
    avatar_id       TEXT,
    avatar_key      BLOB,
    avatar_digest   BLOB,
    avatar_path     TEXT,
    only_admins_send INTEGER NOT NULL DEFAULT 0,
    only_admins_edit INTEGER NOT NULL DEFAULT 0,
    members_json    TEXT NOT NULL DEFAULT '[]',
    left_group      INTEGER NOT NULL DEFAULT 0,
    created_by      TEXT,
    created_at      INTEGER NOT NULL DEFAULT 0,
    distribution_id TEXT NOT NULL
);

CREATE TABLE messages (
    id                TEXT PRIMARY KEY,
    conversation_id   TEXT NOT NULL,
    sender_account_id TEXT NOT NULL,
    sender_device     INTEGER NOT NULL DEFAULT 0,
    outgoing          INTEGER NOT NULL,
    kind              TEXT NOT NULL,
    body              TEXT,
    attachments_json  TEXT NOT NULL DEFAULT '[]',
    location_json     TEXT,
    reply_to_id       TEXT,
    forwarded         INTEGER NOT NULL DEFAULT 0,
    status            TEXT NOT NULL,
    sent_at           INTEGER NOT NULL,
    received_at       INTEGER NOT NULL,
    edited_at         INTEGER,
    deleted           INTEGER NOT NULL DEFAULT 0,
    expires_in        INTEGER NOT NULL DEFAULT 0,
    expires_at        INTEGER,
    system_json       TEXT,
    read_local        INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX messages_conv_sent ON messages(conversation_id, sent_at);
CREATE INDEX messages_expires ON messages(expires_at) WHERE expires_at IS NOT NULL;

CREATE TABLE reactions (
    message_id TEXT NOT NULL,
    account_id TEXT NOT NULL,
    emoji      TEXT NOT NULL,
    ts         INTEGER NOT NULL,
    PRIMARY KEY (message_id, account_id)
);

CREATE TABLE receipts (
    message_id   TEXT NOT NULL,
    account_id   TEXT NOT NULL,
    delivered_at INTEGER,
    read_at      INTEGER,
    PRIMARY KEY (message_id, account_id)
);

-- Logical ids of vault records that must be (re)uploaded.
CREATE TABLE vault_dirty (
    logical_id TEXT PRIMARY KEY
);

CREATE TABLE outbox (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    job             TEXT NOT NULL,
    attempts        INTEGER NOT NULL DEFAULT 0,
    next_attempt_at INTEGER NOT NULL DEFAULT 0,
    created_at      INTEGER NOT NULL
);
"#, r#"
-- Shared contact cards (JSON array of SharedContact).
ALTER TABLE messages ADD COLUMN contacts_json TEXT;
"#, r#"
-- Group rules (see GroupSettings).
ALTER TABLE groups ADD COLUMN disappearing_seconds INTEGER NOT NULL DEFAULT 0;
ALTER TABLE groups ADD COLUMN hide_members INTEGER NOT NULL DEFAULT 0;
ALTER TABLE groups ADD COLUMN member_names TEXT NOT NULL DEFAULT 'number';
"#, r#"
-- @mentions (JSON array of Mention).
ALTER TABLE messages ADD COLUMN mentions_json TEXT;
"#, r#"
-- Groups we are still in were hidden by "delete chat" in older versions: bring them back.
UPDATE conversations SET hidden = 0
 WHERE group_id IN (SELECT group_id FROM groups WHERE left_group = 0);
"#, r#"
-- Pinned and starred messages (this device only; see engine/marks.rs).
CREATE TABLE message_marks (
    message_id      TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL,
    pinned_at       INTEGER,
    pinned_until    INTEGER,
    starred_until   INTEGER
);
CREATE INDEX message_marks_conv ON message_marks(conversation_id);
"#, r#"
-- Group invite links (see engine/invites.rs).
ALTER TABLE groups ADD COLUMN invite_secret TEXT;
ALTER TABLE groups ADD COLUMN invite_on INTEGER NOT NULL DEFAULT 0;
ALTER TABLE groups ADD COLUMN invite_approval INTEGER NOT NULL DEFAULT 0;
ALTER TABLE groups ADD COLUMN join_requests INTEGER NOT NULL DEFAULT 0;
"#, r#"
-- 8: message requests (see engine/requests.rs). Existing conversations count as accepted;
-- new ones are inserted with an explicit value.
ALTER TABLE conversations ADD COLUMN accepted INTEGER NOT NULL DEFAULT 1;
"#, r#"
-- 9: edit history (see engine/edits.rs): earlier versions of an edited message, as a JSON
-- array of EditVersion. A column, not a table: it goes wherever the message goes (deletion,
-- delete-for-everyone, expiry, temporary mode's in-memory tables).
ALTER TABLE messages ADD COLUMN edit_history_json TEXT;
"#, r#"
-- 10: view-once media (see engine/view_once.rs): 0 = normal message, 1 = view-once not opened
-- yet, 2 = opened (received: by us; sent: by the recipient), 3 = expired unopened.
ALTER TABLE messages ADD COLUMN view_once INTEGER NOT NULL DEFAULT 0;
"#, r#"
-- 11: why an outgoing message is still pending or failed (engine/send.rs): "no_devices" (the
-- recipient has no device right now; it is retried) or "account_gone" (deleted account).
ALTER TABLE messages ADD COLUMN status_reason TEXT;
"#, r#"
-- 12: a direct chat we accepted (or that counted as accepted) stays accepted for that person
-- after the conversation is deleted (engine/requests.rs): writing again is not a new request.
ALTER TABLE contacts ADD COLUMN chat_accepted INTEGER NOT NULL DEFAULT 0;
"#, r#"
-- 13: the disappearing timer of a deleted direct chat, applied again when a chat with that person
-- starts (engine/conversations.rs): it stays until the user changes it. NULL: none kept.
ALTER TABLE contacts ADD COLUMN chat_timer INTEGER;
"#, r#"
-- 14: messages are listed in arrival order (received_at, engine/messages.rs get_messages).
CREATE INDEX messages_conv_received ON messages(conversation_id, received_at);
"#];

pub struct Db {
    pub conn: Connection,
}

impl Db {
    /// Opens (or creates) the encrypted database with a 32-byte key.
    pub fn open(path: &Path, key: &[u8; 32]) -> Result<Self> {
        let conn = Connection::open(path)?;
        // Wipe SQLCipher's buffers on free and keep key material out of swap.
        conn.execute_batch("PRAGMA cipher_memory_security = ON;")?;
        conn.execute_batch(&format!("PRAGMA key = \"x'{}'\";", hex::encode(key)))?;
        Self::init(conn)
    }

    /// Whether this database lives only in memory (nothing on disk).
    pub fn is_memory(&self) -> bool {
        self.conn.path().is_none_or(|p| p.is_empty() || p == ":memory:")
    }

    /// Writes this (in-memory) database to `path`, encrypted with `key` from the first byte:
    /// nothing is ever stored unencrypted or under another key.
    pub fn export_to(&self, path: &Path, key: &[u8; 32]) -> Result<()> {
        let p = path.to_string_lossy().replace('\'', "''");
        self.conn.execute_batch(&format!(
            "ATTACH DATABASE '{p}' AS enc KEY \"x'{}'\"; SELECT sqlcipher_export('enc'); DETACH DATABASE enc;",
            hex::encode(key)
        ))?;
        Ok(())
    }

    /// Re-encrypts the whole database file with a new key (SQLCipher rekey).
    pub fn rekey(&self, key: &[u8; 32]) -> Result<()> {
        // Rekeying rewrites every page; fold the WAL in first so no page is left behind
        // under the old key.
        self.conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        self.conn.execute_batch(&format!("PRAGMA rekey = \"x'{}'\";", hex::encode(key)))?;
        self.conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        Ok(())
    }

    /// In-memory database: before the local data is locked with the password (nothing is
    /// written to disk until then), while locked, and in tests.
    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.execute_batch(
            "PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA foreign_keys = ON; PRAGMA secure_delete = ON; \
             PRAGMA temp_store = MEMORY;",
        )?;
        conn.execute_batch("CREATE TABLE IF NOT EXISTS schema_version (v INTEGER NOT NULL);")?;
        let v: i64 = conn
            .query_row("SELECT v FROM schema_version", [], |r| r.get(0))
            .optional()?
            .unwrap_or(0);
        for (i, m) in MIGRATIONS.iter().enumerate().skip(v as usize) {
            let tx = conn.unchecked_transaction()?;
            tx.execute_batch(m)?;
            tx.execute("DELETE FROM schema_version", [])?;
            tx.execute("INSERT INTO schema_version (v) VALUES (?1)", params![(i + 1) as i64])?;
            tx.commit()?;
        }
        Ok(Self { conn })
    }

    pub fn kv_get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM kv WHERE key = ?1", [key], |r| r.get(0))
            .optional()?)
    }

    pub fn kv_set(&self, key: &str, value: &[u8]) -> Result<()> {
        self.conn.execute(
            "INSERT INTO kv (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn kv_delete(&self, key: &str) -> Result<()> {
        self.conn.execute("DELETE FROM kv WHERE key = ?1", [key])?;
        Ok(())
    }

    pub fn kv_get_json<T: serde::de::DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        match self.kv_get(key)? {
            Some(v) => Ok(Some(serde_json::from_slice(&v)?)),
            None => Ok(None),
        }
    }

    pub fn kv_set_json<T: serde::Serialize>(&self, key: &str, value: &T) -> Result<()> {
        self.kv_set(key, &serde_json::to_vec(value)?)
    }

    pub fn kv_get_i64(&self, key: &str) -> Result<Option<i64>> {
        self.kv_get_json(key)
    }

    /// Returns the next value of a persistent counter (starting at `start`).
    pub fn next_counter(&self, key: &str, start: i64) -> Result<i64> {
        let v = self.kv_get_i64(key)?.unwrap_or(start - 1) + 1;
        self.kv_set_json(key, &v)?;
        Ok(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rekey_switches_the_file_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        let (k1, k2) = ([1u8; 32], [2u8; 32]);
        {
            let db = Db::open(&path, &k1).unwrap();
            db.kv_set("a", b"secret value").unwrap();
            db.rekey(&k2).unwrap();
            db.kv_set("b", b"after").unwrap();
        }
        assert!(Db::open(&path, &k1).is_err(), "old key must no longer open the file");
        let db = Db::open(&path, &k2).unwrap();
        assert_eq!(db.kv_get("a").unwrap().unwrap(), b"secret value");
        assert_eq!(db.kv_get("b").unwrap().unwrap(), b"after");
    }
}
