//! Contacts, profiles, contact discovery and safety numbers.

use std::rc::Rc;

use libsignal_protocol::{Fingerprint, IdentityKey};
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::json;

use super::Inner;
use crate::api::{DirectoryEntryDto, PhoneMatchDto, ProfileDto};
use crate::content::{AvatarRef, Content, ProfileData, SyncMessage};
use crate::crypto::{self, b64, unb64};
use crate::error::{CoreError, ErrorCode, Result};
use crate::model::*;
use crate::util::{format_number, normalize_number, normalize_phone, normalize_server_url, now_ms};

const PROFILE_AAD: &[u8] = b"chatapp-profile-v1";
const PROFILE_REFRESH_MS: i64 = 24 * 3600 * 1000;

/// Common country calling codes, used to guess the prefix of national numbers in imports.
const COUNTRY_CODES: &[&str] = &[
    "1", "7", "20", "27", "30", "31", "32", "33", "34", "36", "39", "40", "41", "43", "44", "45", "46", "47",
    "48", "49", "51", "52", "54", "55", "56", "57", "61", "62", "63", "64", "65", "66", "81", "82", "84",
    "86", "90", "91", "92", "212", "213", "351", "352", "353", "354", "355", "356", "357", "358", "359",
    "370", "371", "372", "373", "374", "375", "380", "381", "382", "385", "386", "387", "389", "420", "421",
    "971", "972",
];

impl Inner {
    /// Makes sure a contact row exists (resolving the account number from the directory).
    pub async fn ensure_contact(&self, account_id: &str) -> Result<()> {
        let number: Option<String> = self
            .db
            .borrow()
            .conn
            .query_row("SELECT number FROM contacts WHERE account_id = ?1", [account_id], |r| r.get(0))
            .optional()?;
        if matches!(&number, Some(n) if !n.is_empty()) {
            return Ok(());
        }
        let entry: Result<DirectoryEntryDto> =
            self.session()?.api.get(&format!("/v1/directory/account/{account_id}")).await;
        let now = now_ms();
        match entry {
            Ok(e) => {
                self.db.borrow().conn.execute(
                    "INSERT INTO contacts (account_id, number, updated_at) VALUES (?1, ?2, ?3)
                     ON CONFLICT(account_id) DO UPDATE SET number = excluded.number",
                    params![account_id, e.number, now],
                )?;
            }
            Err(e) if e.is_network() => {
                self.db.borrow().conn.execute(
                    "INSERT OR IGNORE INTO contacts (account_id, number, updated_at) VALUES (?1, '', ?2)",
                    params![account_id, now],
                )?;
            }
            Err(e) => return Err(e),
        }
        Ok(())
    }

    pub fn contact_model(&self, account_id: &str) -> Result<Contact> {
        type Row = (String, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, bool, bool);
        let row: Option<Row> = self
            .db
            .borrow()
            .conn
            .query_row(
                "SELECT number, profile_name, about, avatar_path, local_name, phone, blocked, verified FROM contacts WHERE account_id = ?1",
                [account_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?)),
            )
            .optional()?;
        let (number, profile_name, about, avatar_path, local_name, phone, blocked, verified) =
            row.unwrap_or((String::new(), None, None, None, None, None, false, false));
        let profile_name = profile_name.filter(|n| !n.trim().is_empty());
        let local_name = local_name.filter(|n| !n.trim().is_empty());
        let display_name = local_name
            .clone()
            .or_else(|| profile_name.clone())
            .unwrap_or_else(|| Self::number_label(&number));
        Ok(Contact {
            account_id: account_id.to_string(),
            number,
            profile_name,
            local_name,
            display_name,
            about: about.filter(|a| !a.is_empty()),
            avatar_path,
            phone,
            blocked,
            verified,
        })
    }

    /// Contact card for sharing. Uses the name the person chose for their profile, never our
    /// private local nickname for them.
    pub fn shared_contact(&self, account_id: &str) -> Result<SharedContact> {
        let s = self.session()?;
        if account_id == s.account.account_id {
            let name = self.get_profile()?.name;
            let name = if name.trim().is_empty() { Self::number_label(&s.account.number) } else { name };
            return Ok(SharedContact { account_id: account_id.to_string(), number: s.account.number.clone(), name });
        }
        let c = self.contact_model(account_id)?;
        if c.number.is_empty() {
            return Err(CoreError::not_found("contact not found"));
        }
        let name = c.profile_name.clone().unwrap_or_else(|| Self::number_label(&c.number));
        Ok(SharedContact { account_id: c.account_id, number: c.number, name })
    }

    pub fn display_name(&self, account_id: &str) -> String {
        self.contact_model(account_id).map(|c| c.display_name).unwrap_or_else(|_| "Unknown".into())
    }

    pub fn is_blocked(&self, account_id: &str) -> bool {
        self.db
            .borrow()
            .conn
            .query_row("SELECT blocked FROM contacts WHERE account_id = ?1", [account_id], |r| r.get(0))
            .unwrap_or(false)
    }

    fn emit_contact(&self, account_id: &str) {
        if let Ok(c) = self.contact_model(account_id) {
            self.emit("contact_updated", &c);
        }
        // A profile change may mean they are back on a new device.
        self.wake_no_device_jobs(Some(account_id));
        if let Ok(Some(conv)) = self.conversation_id_for_peer(account_id) {
            self.emit_conversation(&conv);
        }
    }

    pub fn list_contacts(&self) -> Result<Vec<Contact>> {
        let me = self.me()?;
        let ids: Vec<String> = self
            .db
            .borrow()
            .conn
            .prepare("SELECT account_id FROM contacts WHERE is_contact = 1 AND account_id != ?1")?
            .query_map([&me], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let mut out: Vec<Contact> = ids.iter().map(|id| self.contact_model(id)).collect::<Result<_>>()?;
        out.sort_by_key(|c| c.display_name.to_lowercase());
        Ok(out)
    }

    pub async fn add_contact_by_number(self: &Rc<Self>, number: &str, name: Option<String>) -> Result<Contact> {
        let number = normalize_number(number).ok_or_else(|| CoreError::invalid("invalid account number"))?;
        let s = self.session()?;
        let entry: DirectoryEntryDto = s
            .api
            .get(&format!("/v1/directory/number/{number}"))
            .await
            .map_err(|e| if e.code == ErrorCode::NotFound { CoreError::not_found("no account with this number") } else { e })?;
        let name = name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
        self.db.borrow().conn.execute(
            "INSERT INTO contacts (account_id, number, local_name, is_contact, updated_at) VALUES (?1, ?2, ?3, 1, ?4)
             ON CONFLICT(account_id) DO UPDATE SET number = excluded.number, is_contact = 1,
                local_name = COALESCE(excluded.local_name, contacts.local_name), updated_at = excluded.updated_at",
            params![entry.account_id, entry.number, name, now_ms()],
        )?;
        self.vault_put_contact(&entry.account_id);
        self.emit_contact(&entry.account_id);
        self.contact_model(&entry.account_id)
    }

    /// Looks up an account by account number or username (without adding it as a contact).
    /// Returns None when nothing matches.
    pub async fn find_account(&self, query: &str) -> Result<Option<FoundAccount>> {
        let q = query.trim();
        let s = self.session()?;
        let path = if let Some(number) = normalize_number(q) {
            format!("/v1/directory/number/{number}")
        } else if let Some(phone) = self.phone_query(q).await {
            // Phone numbers are looked up by hash, and only match accounts that keep their
            // verified number findable.
            use base64::Engine as _;
            let hash = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(crypto::sha256(phone.as_bytes()));
            format!("/v1/directory/phone/{hash}")
        } else {
            let u = q.trim_start_matches('@').to_lowercase();
            let valid = u.len() >= 3
                && u.len() <= 32
                && u.chars().next().map(|c| c.is_ascii_lowercase()).unwrap_or(false)
                && u.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.');
            if !valid {
                return Ok(None);
            }
            format!("/v1/directory/username/{u}")
        };
        let entry: DirectoryEntryDto = match s.api.get(&path).await {
            Ok(e) => e,
            Err(e) if e.code == ErrorCode::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        let known = self.contact_model(&entry.account_id)?;
        Ok(Some(FoundAccount {
            is_me: entry.account_id == s.account.account_id,
            display_name: if known.number.is_empty() {
                entry.username.clone().map(|u| format!("@{u}")).unwrap_or_else(|| Self::number_label(&entry.number))
            } else {
                known.display_name
            },
            avatar_path: known.avatar_path,
            account_id: entry.account_id,
            number: entry.number,
            username: entry.username,
        }))
    }

    /// Adds a contact found by account number or username.
    pub async fn add_contact_by_query(self: &Rc<Self>, query: &str, name: Option<String>) -> Result<Contact> {
        let found = self.find_account(query).await?.ok_or_else(|| CoreError::not_found("no account found"))?;
        self.add_contact_by_number(&found.number, name).await
    }

    pub fn list_blocked(&self) -> Result<Vec<Contact>> {
        let ids: Vec<String> = self
            .db
            .borrow()
            .conn
            .prepare("SELECT account_id FROM contacts WHERE blocked = 1")?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let mut out: Vec<Contact> = ids.iter().map(|id| self.contact_model(id)).collect::<Result<_>>()?;
        out.sort_by_key(|c| c.display_name.to_lowercase());
        Ok(out)
    }

    pub fn rename_contact(&self, account_id: &str, name: Option<String>) -> Result<Contact> {
        let name = name.map(|n| n.trim().chars().take(100).collect::<String>()).filter(|n| !n.is_empty());
        self.db.borrow().conn.execute(
            "UPDATE contacts SET local_name = ?2, is_contact = 1, updated_at = ?3 WHERE account_id = ?1",
            params![account_id, name, now_ms()],
        )?;
        self.vault_put_contact(account_id);
        self.emit_contact(account_id);
        self.contact_model(account_id)
    }

    pub fn set_blocked(&self, account_id: &str, blocked: bool) -> Result<Contact> {
        if account_id == self.me()? {
            return Err(CoreError::invalid("you cannot block yourself"));
        }
        let was_blocked = self.is_blocked(account_id);
        // Only someone we shared an accepted chat or group with can have our profile key
        // (engine/requests.rs): blocking a stranger's message request needs no rotation.
        let had_key = self.shares_accepted_chat(account_id);
        self.db.borrow().conn.execute(
            // Blocking also forgets an earlier accepted chat (engine/requests.rs).
            "UPDATE contacts SET blocked = ?2, chat_accepted = CASE WHEN ?2 THEN 0 ELSE chat_accepted END, updated_at = ?3 WHERE account_id = ?1",
            params![account_id, blocked, now_ms()],
        )?;
        if blocked && !was_blocked && had_key {
            // They keep no access to our future profile, avatar, presence or sealed delivery.
            self.request_profile_key_rotation()?;
        }
        self.vault_put_contact(account_id);
        self.emit_contact(account_id);
        self.contact_model(account_id)
    }

    pub fn set_verified(&self, account_id: &str, verified: bool) -> Result<Contact> {
        self.db.borrow().conn.execute(
            "UPDATE contacts SET verified = ?2, updated_at = ?3 WHERE account_id = ?1",
            params![account_id, verified, now_ms()],
        )?;
        self.vault_put_contact(account_id);
        self.emit_contact(account_id);
        self.contact_model(account_id)
    }

    pub fn my_contact_link(&self) -> Result<String> {
        let s = self.session()?;
        let server: String = url::form_urlencoded::byte_serialize(s.account.server_url.as_bytes()).collect();
        Ok(format!("chatapp://add?n={}&s={}", s.account.number, server))
    }

    pub async fn add_contact_from_link(self: &Rc<Self>, link: &str) -> Result<Contact> {
        let link = link.trim();
        if super::username_link::is_username_link(link) {
            return self.add_contact_from_username_link(link).await;
        }
        let (number, server) = if let Ok(url) = url::Url::parse(link) {
            let mut number = None;
            let mut server = None;
            for (k, v) in url.query_pairs() {
                match k.as_ref() {
                    "n" => number = Some(v.to_string()),
                    "s" => server = Some(v.to_string()),
                    _ => {}
                }
            }
            (number.ok_or_else(|| CoreError::invalid("not a contact link"))?, server)
        } else {
            (link.to_string(), None)
        };
        if let Some(server) = server {
            let mine = self.session()?.account.server_url.clone();
            if normalize_server_url(&server).as_deref() != normalize_server_url(&mine).as_deref() {
                return Err(CoreError::new(
                    ErrorCode::DifferentServer,
                    format!("this contact uses another server ({server})"),
                ));
            }
        }
        self.add_contact_by_number(&number, None).await
    }

    pub async fn safety_number(&self, account_id: &str) -> Result<SafetyNumber> {
        let s = self.session()?;
        let stored: Option<Vec<u8>> = self
            .db
            .borrow()
            .conn
            .query_row("SELECT identity_key FROM identities WHERE account_id = ?1", [account_id], |r| r.get(0))
            .optional()?;
        let remote = match stored {
            Some(k) => IdentityKey::decode(&k)?,
            None => {
                let e: DirectoryEntryDto = s.api.get(&format!("/v1/directory/account/{account_id}")).await?;
                IdentityKey::decode(&unb64(&e.identity_key)?)?
            }
        };
        let local_id = uuid::Uuid::parse_str(&s.account.account_id).map_err(|e| CoreError::internal(e.to_string()))?;
        let remote_id = uuid::Uuid::parse_str(account_id).map_err(|e| CoreError::internal(e.to_string()))?;
        let fp = Fingerprint::new(
            2,
            5200,
            local_id.as_bytes(),
            s.keys.identity.identity_key(),
            remote_id.as_bytes(),
            &remote,
        )
        .map_err(|e| CoreError::internal(e.to_string()))?;
        let digits = fp.display_string().map_err(|e| CoreError::internal(e.to_string()))?;
        let grouped = digits
            .as_bytes()
            .chunks(5)
            .map(|c| std::str::from_utf8(c).unwrap_or_default())
            .collect::<Vec<_>>()
            .join(" ");
        Ok(SafetyNumber {
            digits: grouped,
            qr_payload: b64(fp.scannable.serialize().map_err(|e| CoreError::internal(e.to_string()))?),
            verified: self.contact_model(account_id)?.verified,
        })
    }

    // --- address book import --------------------------------------------------

    pub async fn import_contacts(self: &Rc<Self>, path: &str) -> Result<ContactImportResult> {
        let data = tokio::fs::read(path).await?;
        if data.len() > 20 * 1024 * 1024 {
            return Err(CoreError::invalid("file is too large"));
        }
        let text = String::from_utf8_lossy(&data);
        let entries = if text.to_ascii_uppercase().contains("BEGIN:VCARD") { parse_vcards(&text) } else { parse_csv(&text) };
        let total = entries.len();
        let prefix = self.my_country_prefix().await;
        let mut by_hash: std::collections::HashMap<String, (String, String)> = std::collections::HashMap::new();
        for (name, phones) in &entries {
            for p in phones {
                if let Some(e164) = normalize_phone(p, prefix.as_deref()) {
                    by_hash.insert(b64(crypto::sha256(e164.as_bytes())), (name.clone(), e164));
                }
            }
        }
        let with_phone = entries.iter().filter(|(_, p)| !p.is_empty()).count();
        let hashes: Vec<String> = by_hash.keys().cloned().collect();
        let s = self.session()?;
        let me = s.account.account_id.clone();
        let mut matched = 0;
        for chunk in hashes.chunks(1000) {
            #[derive(Deserialize)]
            struct Resp {
                matches: Vec<PhoneMatchDto>,
            }
            let r: Resp = s.api.post("/v1/directory/phones", &json!({ "hashes": chunk })).await?;
            for m in r.matches {
                if m.account_id == me {
                    continue;
                }
                let Some((name, phone)) = by_hash.get(&m.hash) else { continue };
                let name = Some(name.clone()).filter(|n| !n.is_empty());
                self.db.borrow().conn.execute(
                    "INSERT INTO contacts (account_id, number, local_name, phone, is_contact, updated_at) VALUES (?1, ?2, ?3, ?4, 1, ?5)
                     ON CONFLICT(account_id) DO UPDATE SET number = excluded.number, phone = excluded.phone, is_contact = 1,
                        local_name = COALESCE(contacts.local_name, excluded.local_name), updated_at = excluded.updated_at",
                    params![m.account_id, m.number, name, phone, now_ms()],
                )?;
                self.vault_put_contact(&m.account_id);
                self.emit_contact(&m.account_id);
                matched += 1;
            }
        }
        Ok(ContactImportResult { total, with_phone, matched })
    }

    /// A search text that is a phone number (not a 16-digit account number), in E.164.
    /// National numbers ("07...") use the country of our own verified number.
    async fn phone_query(&self, q: &str) -> Option<String> {
        let digits = q.chars().filter(|c| c.is_ascii_digit()).count();
        let phone_chars = q.chars().all(|c| c.is_ascii_digit() || " +-().".contains(c));
        if !phone_chars || !(6..=15).contains(&digits) {
            return None;
        }
        let prefix = if q.trim_start().starts_with('0') && !q.trim_start().starts_with("00") {
            self.my_country_prefix().await
        } else {
            None
        };
        normalize_phone(q, prefix.as_deref())
    }

    async fn my_country_prefix(&self) -> Option<String> {
        let dto: crate::api::AccountDto = self.session().ok()?.api.get("/v1/account").await.ok()?;
        let phone = dto.phone?;
        let digits = phone.strip_prefix('+')?;
        COUNTRY_CODES
            .iter()
            .filter(|c| digits.starts_with(**c))
            .max_by_key(|c| c.len())
            .map(|c| format!("+{c}"))
    }

    // --- profiles ---------------------------------------------------------------

    pub fn get_profile(&self) -> Result<Profile> {
        Ok(self.db.borrow().kv_get_json("profile")?.unwrap_or_default())
    }

    fn profile_avatar_ref(&self) -> Result<Option<AvatarRef>> {
        self.db.borrow().kv_get_json("profile_avatar")
    }

    pub async fn publish_profile(&self) -> Result<()> {
        self.publish_profile_with(&self.profile_key()?).await
    }

    /// Publishes our profile encrypted with `key` (engine/profile_key.rs uses the next key).
    pub(super) async fn publish_profile_with(&self, key: &[u8; 32]) -> Result<()> {
        let s = self.session()?;
        let p = self.get_profile()?;
        let avatar = self.profile_avatar_ref()?;
        let data = ProfileData { name: p.name, about: p.about, avatar: avatar.clone() };
        let sealed = crypto::aead_seal(key, &serde_json::to_vec(&data)?, PROFILE_AAD)?;
        let _: serde_json::Value = s
            .api
            .put("/v1/profile", &json!({ "data": b64(sealed), "avatar_id": avatar.map(|a| a.cdn_id) }))
            .await?;
        Ok(())
    }

    pub async fn set_profile(&self, req: SetProfileRequest) -> Result<Profile> {
        let mut p = self.get_profile()?;
        if let Some(n) = req.name {
            let n: String = n.trim().chars().take(64).collect();
            p.name = n;
        }
        if let Some(a) = req.about {
            p.about = a.trim().chars().take(140).collect();
        }
        let mut avatar_path = req.avatar_path;
        let mut temp_file = None;
        if let Some(data) = req.avatar_data_base64.filter(|d| !d.is_empty()) {
            let data = data.split_once("base64,").map(|(_, rest)| rest.to_string()).unwrap_or(data);
            let tmp = self.media_dir().join(format!("avatar-upload-{}", crate::util::new_id()));
            self.write_media_file(&tmp, unb64(&data)?).await?;
            avatar_path = Some(tmp.to_string_lossy().to_string());
            temp_file = Some(tmp);
        }
        let result = self.apply_profile_changes(p, avatar_path).await;
        if let Some(t) = temp_file {
            let _ = std::fs::remove_file(t);
        }
        result
    }

    async fn apply_profile_changes(&self, mut p: Profile, avatar_path: Option<String>) -> Result<Profile> {
        if let Some(path) = avatar_path {
            if path.is_empty() {
                p.avatar_path = None;
                self.db.borrow().kv_delete("profile_avatar")?;
            } else {
                let (local, avatar) = self.upload_avatar_from_path(&path, "self").await?;
                p.avatar_path = Some(local);
                self.db.borrow().kv_set_json("profile_avatar", &avatar)?;
            }
        }
        self.db.borrow().kv_set_json("profile", &p)?;
        self.publish_profile().await?;
        self.emit("profile_updated", &p);
        self.enqueue(&super::send::Job::OwnDevices { content: Content::Sync(SyncMessage::ProfileUpdated) })?;
        self.announce_profile()?;
        Ok(p)
    }

    /// Tells everyone we talk to (direct chats and group members) to refetch our profile.
    /// Message requests and pending group invites are left out (engine/requests.rs).
    pub(super) fn announce_profile(&self) -> Result<()> {
        let me = self.me()?;
        let key = self.profile_key_b64()?;
        let peers: Vec<String> = self
            .db
            .borrow()
            .conn
            .prepare("SELECT peer_account_id FROM conversations WHERE peer_account_id IS NOT NULL")?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let mut targets: std::collections::BTreeSet<String> = peers.into_iter().filter(|p| self.trusts_account(p)).collect();
        let groups: Vec<String> = self
            .db
            .borrow()
            .conn
            .prepare(
                "SELECT g.group_id FROM groups g JOIN conversations c ON c.group_id = g.group_id
                 WHERE g.left_group = 0 AND c.accepted = 1",
            )?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        for g in groups {
            targets.extend(self.group_member_ids(&g)?);
        }
        for account_id in targets.into_iter().filter(|a| *a != me && !self.is_blocked(a)) {
            self.enqueue(&super::send::Job::Account {
                account_id,
                content: Content::ProfileUpdated { profile_key: key.clone() },
            })?;
        }
        Ok(())
    }

    /// Refetches a contact's profile in the background when our copy is older than an hour.
    pub fn refresh_profile_if_stale(self: &Rc<Self>, account_id: &str) {
        let fetched: i64 = self
            .db
            .borrow()
            .conn
            .query_row("SELECT profile_fetched_at FROM contacts WHERE account_id = ?1", [account_id], |r| r.get(0))
            .unwrap_or(0);
        if now_ms() - fetched < 3600 * 1000 {
            return;
        }
        let me = self.clone();
        let id = account_id.to_string();
        tokio::task::spawn_local(async move {
            if let Err(e) = me.fetch_profile(&id).await {
                log::debug!("profile refresh {id}: {e}");
            }
        });
    }

    /// Stores a contact's profile key and refreshes their profile when it changed.
    pub async fn update_profile_key(self: &Rc<Self>, account_id: &str, key_b64: &str) {
        let Ok(key) = unb64(key_b64) else { return };
        if key.len() != 32 {
            return;
        }
        let current: Option<Vec<u8>> = self
            .db
            .borrow()
            .conn
            .query_row("SELECT profile_key FROM contacts WHERE account_id = ?1", [account_id], |r| r.get(0))
            .optional()
            .ok()
            .flatten();
        if current.as_deref() == Some(&key[..]) {
            return;
        }
        let _ = self.ensure_contact(account_id).await;
        let _ = self.db.borrow().conn.execute(
            "UPDATE contacts SET profile_key = ?2 WHERE account_id = ?1",
            params![account_id, key],
        );
        self.vault_put_contact(account_id);
        // Their access key is now known: their presence can be watched (if the chat is accepted).
        self.send_presence_subscription();
        let me = self.clone();
        let id = account_id.to_string();
        tokio::task::spawn_local(async move {
            if let Err(e) = me.fetch_profile(&id).await {
                log::warn!("profile {id}: {e}");
            }
        });
    }

    /// Downloads and decrypts a profile (ours or a contact's).
    pub async fn fetch_profile(&self, account_id: &str) -> Result<()> {
        let s = self.session()?;
        let is_me = account_id == s.account.account_id;
        let key: [u8; 32] = if is_me {
            self.profile_key()?
        } else {
            let k: Option<Vec<u8>> = self
                .db
                .borrow()
                .conn
                .query_row("SELECT profile_key FROM contacts WHERE account_id = ?1", [account_id], |r| r.get(0))
                .optional()?
                .flatten();
            match k {
                Some(k) => crypto::key32(&k)?,
                None => return Ok(()),
            }
        };
        let dto: ProfileDto = match s.api.get(&format!("/v1/profile/{account_id}")).await {
            Ok(d) => d,
            Err(e) if e.code == ErrorCode::NotFound => return Ok(()),
            Err(e) => return Err(e),
        };
        let plain = crypto::aead_open(&key, &unb64(&dto.data)?, PROFILE_AAD)?;
        let data: ProfileData = serde_json::from_slice(&plain)?;
        if is_me {
            let mut p = self.get_profile()?;
            p.name = data.name;
            p.about = data.about;
            if data.avatar != self.profile_avatar_ref()? {
                match &data.avatar {
                    Some(a) => {
                        p.avatar_path = Some(self.download_avatar(a, "self").await?);
                        self.db.borrow().kv_set_json("profile_avatar", a)?;
                    }
                    None => {
                        p.avatar_path = None;
                        self.db.borrow().kv_delete("profile_avatar")?;
                    }
                }
            }
            self.db.borrow().kv_set_json("profile", &p)?;
            self.emit("profile_updated", &p);
            return Ok(());
        }
        let current_avatar: Option<String> = self
            .db
            .borrow()
            .conn
            .query_row("SELECT avatar_id FROM contacts WHERE account_id = ?1", [account_id], |r| r.get(0))
            .optional()?
            .flatten();
        let (avatar_id, avatar_path) = match &data.avatar {
            Some(a) if current_avatar.as_deref() == Some(a.cdn_id.as_str()) => (Some(a.cdn_id.clone()), None),
            // A stranger's avatar (message request) is neither downloaded nor decoded until the
            // chat is accepted; the profile is fetched again then.
            Some(_) if !self.shares_accepted_chat(account_id) => (current_avatar.clone(), None),
            Some(a) => (Some(a.cdn_id.clone()), Some(self.download_avatar(a, account_id).await?)),
            None => (None, None),
        };
        {
            let db = self.db.borrow();
            db.conn.execute(
                "UPDATE contacts SET profile_name = ?2, about = ?3, profile_fetched_at = ?4 WHERE account_id = ?1",
                params![account_id, crate::sanitize::clean_display_text(&data.name, 64), crate::sanitize::clean_display_text(&data.about, 140), now_ms()],
            )?;
            if avatar_id.is_none() {
                db.conn.execute(
                    "UPDATE contacts SET avatar_id = NULL, avatar_path = NULL WHERE account_id = ?1",
                    [account_id],
                )?;
            } else if let Some(path) = avatar_path {
                db.conn.execute(
                    "UPDATE contacts SET avatar_id = ?2, avatar_path = ?3 WHERE account_id = ?1",
                    params![account_id, avatar_id, path],
                )?;
            }
        }
        self.emit_contact(account_id);
        Ok(())
    }

    pub async fn refresh_stale_profiles(&self) {
        let cutoff = now_ms() - PROFILE_REFRESH_MS;
        let ids: Vec<String> = self
            .db
            .borrow()
            .conn
            .prepare("SELECT account_id FROM contacts WHERE profile_key IS NOT NULL AND profile_fetched_at < ?1 LIMIT 200")
            .and_then(|mut st| st.query_map([cutoff], |r| r.get(0))?.collect())
            .unwrap_or_default();
        for id in ids {
            if let Err(e) = self.fetch_profile(&id).await {
                log::debug!("profile refresh {id}: {e}");
                if e.is_network() {
                    break;
                }
            }
        }
    }

    pub fn formatted_number(&self) -> Result<String> {
        Ok(format_number(&self.session()?.account.number))
    }
}

/// Parses vCards into (name, phones).
fn parse_vcards(text: &str) -> Vec<(String, Vec<String>)> {
    let mut out = Vec::new();
    let mut name = String::new();
    let mut phones = Vec::new();
    let mut in_card = false;
    // Unfold continuation lines (RFC 6350 §3.2).
    let unfolded = text.replace("\r\n ", "").replace("\n ", "").replace("\r\n\t", "").replace("\n\t", "");
    for line in unfolded.lines() {
        let upper = line.to_ascii_uppercase();
        if upper.starts_with("BEGIN:VCARD") {
            in_card = true;
            name.clear();
            phones.clear();
        } else if upper.starts_with("END:VCARD") {
            if in_card {
                out.push((name.clone(), phones.clone()));
            }
            in_card = false;
        } else if in_card {
            let Some((key, value)) = line.split_once(':') else { continue };
            let key = key.to_ascii_uppercase();
            if key == "FN" || key.starts_with("FN;") {
                name = value.trim().to_string();
            } else if (key == "N" || key.starts_with("N;")) && name.is_empty() {
                let parts: Vec<&str> = value.split(';').collect();
                name = format!("{} {}", parts.get(1).unwrap_or(&""), parts.first().unwrap_or(&"")).trim().to_string();
            } else if key == "TEL" || key.starts_with("TEL;") || key.contains(".TEL") {
                phones.push(value.trim().trim_start_matches("tel:").to_string());
            }
        }
    }
    out
}

/// Parses a CSV address book: first column containing letters is the name,
/// every cell that looks like a phone number is a phone.
fn parse_csv(text: &str) -> Vec<(String, Vec<String>)> {
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let cells: Vec<String> = line.split([',', ';', '\t']).map(|c| c.trim().trim_matches('"').to_string()).collect();
        let phones: Vec<String> = cells
            .iter()
            .filter(|c| {
                let digits = c.chars().filter(|ch| ch.is_ascii_digit()).count();
                digits >= 7 && c.chars().all(|ch| ch.is_ascii_digit() || " +-()./".contains(ch))
            })
            .cloned()
            .collect();
        if phones.is_empty() {
            if i == 0 {
                continue; // header
            }
            continue;
        }
        let name = cells.iter().find(|c| c.chars().any(|ch| ch.is_alphabetic())).cloned().unwrap_or_default();
        out.push((name, phones));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vcard_parsing() {
        let v = "BEGIN:VCARD\nVERSION:3.0\nFN:Ana Pop\nTEL;TYPE=CELL:+40 722 123 456\nEND:VCARD\nBEGIN:VCARD\nN:Rossi;Mario;;;\nTEL:0039 347 1234567\nEND:VCARD\n";
        let r = parse_vcards(v);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].0, "Ana Pop");
        assert_eq!(r[1].0, "Mario Rossi");
        assert_eq!(r[1].1, vec!["0039 347 1234567"]);
    }

    #[test]
    fn csv_parsing() {
        let c = "Name,Phone\nIon Ionescu,+40722000111\nNo phone,\n";
        let r = parse_csv(c);
        assert_eq!(r, vec![("Ion Ionescu".to_string(), vec!["+40722000111".to_string()])]);
    }
}
