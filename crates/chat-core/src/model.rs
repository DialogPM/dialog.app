//! Types exposed to the UI. They mirror `app/src/lib/types.ts` field by field.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerInfo {
    pub url: String,
    pub name: String,
    pub version: String,
    pub registration_enabled: bool,
    pub phone_verification_enabled: bool,
    pub max_attachment_bytes: i64,
    pub max_devices: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AccountInfo {
    pub account_id: String,
    pub number: String,
    pub device_id: u32,
    pub server_url: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AppState {
    pub status: &'static str,
    pub account: Option<AccountInfo>,
    pub default_server_url: String,
    /// Recovery phrase of a just-created account that the user has not confirmed yet.
    pub pending_mnemonic: Option<Vec<String>>,
    /// The local database is locked with the account password (always true once set up;
    /// false right after linking a device or for data from older versions).
    pub local_protected: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct RegisterResult {
    pub account: AccountInfo,
    pub mnemonic: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Profile {
    pub name: String,
    pub about: String,
    pub avatar_path: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Contact {
    pub account_id: String,
    pub number: String,
    pub profile_name: Option<String>,
    pub local_name: Option<String>,
    pub display_name: String,
    pub about: Option<String>,
    pub avatar_path: Option<String>,
    pub phone: Option<String>,
    pub blocked: bool,
    pub verified: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct MessagePreview {
    pub id: String,
    pub kind: String,
    pub text: Option<String>,
    pub sender_name: Option<String>,
    pub outgoing: bool,
    pub status: String,
    pub sent_at: i64,
    pub deleted: bool,
    /// For system messages: the event, so the chat list can describe it.
    pub system: Option<SystemEvent>,
    /// View-once photo/video (engine/view_once.rs): the chat list shows a placeholder.
    pub view_once: bool,
    /// The message's media was deleted by the 30-day retention (engine/retention.rs): the
    /// chat list shows a placeholder instead of "Photo" / "Voice message".
    pub media_expired: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Conversation {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub avatar_path: Option<String>,
    pub peer_account_id: Option<String>,
    pub peer_number: Option<String>,
    pub group_id: Option<String>,
    pub member_count: Option<i64>,
    pub is_admin: bool,
    pub left: bool,
    pub last_message: Option<MessagePreview>,
    pub unread_count: i64,
    /// Unread messages that mention us.
    pub unread_mentions: i64,
    pub muted_until: Option<i64>,
    pub archived: bool,
    pub pinned: bool,
    pub disappearing_seconds: i64,
    pub blocked: bool,
    pub draft: Option<String>,
    pub updated_at: i64,
    /// False for a message request (a stranger wrote first) or a group invite from a stranger
    /// that the user has not accepted yet (see engine/requests.rs).
    pub accepted: bool,
    /// Direct chats: the peer is in our contacts. Calls with anyone else go through the TURN
    /// relay only, so they do not learn our IP address.
    pub peer_is_contact: bool,
}

/// Attachment as stored locally (superset of the UI type; keys are never sent to the UI).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StoredAttachment {
    pub id: String,
    pub mime: String,
    pub file_name: String,
    pub size: i64,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub duration_ms: Option<i64>,
    pub waveform: Option<Vec<f32>>,
    pub thumbnail: Option<String>,
    pub local_path: Option<String>,
    pub download_state: String,
    /// Server attachment id (None until uploaded).
    pub cdn_id: Option<String>,
    /// base64 64-byte key
    pub key: Option<String>,
    /// base64 sha256 digest of the ciphertext
    pub digest: Option<String>,
    #[serde(default)]
    pub kind: String,
    /// When the mandatory media retention (engine/retention.rs) erased this attachment: the
    /// file, key, server pointer and preview are gone, only the kind, type and size remain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expired_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Attachment {
    pub id: String,
    pub mime: String,
    pub file_name: String,
    pub size: i64,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub duration_ms: Option<i64>,
    pub waveform: Option<Vec<f32>>,
    pub thumbnail: Option<String>,
    pub local_path: Option<String>,
    pub download_state: String,
    pub progress: Option<f32>,
    /// Set when the media was deleted by the 30-day retention (download_state "expired").
    pub expired_at: Option<i64>,
}

impl From<&StoredAttachment> for Attachment {
    fn from(a: &StoredAttachment) -> Self {
        Self {
            id: a.id.clone(),
            mime: a.mime.clone(),
            file_name: a.file_name.clone(),
            size: a.size,
            width: a.width,
            height: a.height,
            duration_ms: a.duration_ms,
            waveform: a.waveform.clone(),
            thumbnail: a.thumbnail.clone(),
            local_path: a.local_path.clone(),
            download_state: a.download_state.clone(),
            progress: None,
            expired_at: a.expired_at,
        }
    }
}

/// A contact card shared in a message (like a vCard): enough to start a chat with them.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SharedContact {
    pub account_id: String,
    /// 16-digit account number.
    pub number: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Location {
    pub lat: f64,
    pub lng: f64,
    pub name: Option<String>,
    pub address: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Reaction {
    pub emoji: String,
    pub account_id: String,
    pub sender_name: String,
    pub is_me: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct QuotedMessage {
    pub id: String,
    pub sender_name: String,
    pub kind: String,
    pub text: Option<String>,
    pub thumbnail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemEvent {
    #[serde(rename = "type")]
    pub kind: String,
    pub actor_name: Option<String>,
    pub target_names: Vec<String>,
    pub value: Option<String>,
}

/// System event as stored: account ids are resolved to names at read time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredSystemEvent {
    #[serde(rename = "type")]
    pub kind: String,
    pub actor: Option<String>,
    #[serde(default)]
    pub targets: Vec<String>,
    pub value: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Message {
    pub id: String,
    pub conversation_id: String,
    pub sender_account_id: String,
    pub sender_name: String,
    pub outgoing: bool,
    pub kind: String,
    pub text: Option<String>,
    pub attachments: Vec<Attachment>,
    pub location: Option<Location>,
    pub contacts: Vec<SharedContact>,
    pub reply_to: Option<QuotedMessage>,
    pub reactions: Vec<Reaction>,
    pub status: String,
    /// Outgoing only: why the message is still pending or failed. "no_devices": the recipient
    /// has no device right now, it is sent when one comes back (status "pending"; "failed"
    /// after a week). "account_gone": the recipient account was deleted (status "failed").
    pub status_reason: Option<String>,
    pub sent_at: i64,
    pub received_at: i64,
    pub edited_at: Option<i64>,
    pub deleted: bool,
    pub forwarded: bool,
    pub expires_at: Option<i64>,
    pub system: Option<SystemEvent>,
    pub mentions: Vec<Mention>,
    /// We are mentioned in this message.
    pub mentions_me: bool,
    /// Pinned in its conversation (this device only) until this time (ms).
    pub pinned_until: Option<i64>,
    /// Starred (this device only, for a week).
    pub starred: bool,
    /// View-once media (engine/view_once.rs): "unopened", "opened" or "expired"; null for
    /// ordinary messages. The media of such a message is never in `attachments[].local_path`
    /// or `thumbnail`: it is shown only through `open_view_once`.
    pub view_once: Option<String>,
}

/// An @mention: `text[start..start+length]` (UTF-16 code units, as in JS and Dart strings)
/// is the name shown for `account_id`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Mention {
    pub account_id: String,
    pub start: u32,
    pub length: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct MessageReceipt {
    pub account_id: String,
    pub display_name: String,
    pub avatar_path: Option<String>,
    pub status: String,
    pub delivered_at: Option<i64>,
    pub read_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MessageInfo {
    pub message: Message,
    pub receipts: Vec<MessageReceipt>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GroupSettings {
    pub only_admins_send: bool,
    pub only_admins_edit: bool,
    /// Disappearing-messages timer of the whole group (0 = off); only admins change it.
    #[serde(default)]
    pub disappearing_seconds: i64,
    /// Members (other than admins) see only how many members there are.
    #[serde(default)]
    pub hide_members: bool,
    /// "number": members are shown by account number / name as usual. "first_name": every
    /// member must have a profile name and only its first word is shown.
    #[serde(default = "default_member_names")]
    pub member_names: String,
}

fn default_member_names() -> String {
    "number".into()
}

/// Changes to group settings; omitted fields keep their value.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GroupSettingsPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub only_admins_send: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub only_admins_edit: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disappearing_seconds: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hide_members: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member_names: Option<String>,
}

impl GroupSettingsPatch {
    pub fn validate(&self) -> crate::error::Result<()> {
        if let Some(d) = self.disappearing_seconds {
            if !(0..=365 * 86400).contains(&d) {
                return Err(crate::error::CoreError::invalid("invalid disappearing timer"));
            }
        }
        if let Some(n) = &self.member_names {
            if n != "number" && n != "first_name" {
                return Err(crate::error::CoreError::invalid("member_names must be \"number\" or \"first_name\""));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct GroupMember {
    pub account_id: String,
    pub number: String,
    pub display_name: String,
    pub avatar_path: Option<String>,
    pub role: String,
    pub is_me: bool,
    /// Confirmed end to end by an admin (false: listed only by the server).
    pub verified: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct GroupInfo {
    pub conversation_id: String,
    pub group_id: String,
    pub name: String,
    pub description: String,
    pub avatar_path: Option<String>,
    /// All members for admins; only ourselves when the admins hid the member list.
    pub members: Vec<GroupMember>,
    pub member_count: usize,
    /// The member list is hidden from us (see GroupSettings::hide_members).
    pub members_hidden: bool,
    pub my_role: Option<String>,
    pub settings: GroupSettings,
    pub created_by: Option<String>,
    pub created_at: i64,
    /// The invite link (admins only; null when there is none).
    pub invite_link: Option<String>,
    /// Joining with the link needs an admin's approval.
    pub invite_approval: bool,
    /// Pending join requests (admins only).
    pub join_requests: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeviceInfo {
    pub device_id: u32,
    pub name: String,
    pub platform: String,
    pub created_at: i64,
    pub last_seen_at: i64,
    pub current: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PrivacySettings {
    pub last_seen: String,
    pub read_receipts: bool,
    pub typing_indicators: bool,
}

impl Default for PrivacySettings {
    fn default() -> Self {
        Self { last_seen: "everyone".into(), read_receipts: true, typing_indicators: true }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct AccountDetails {
    pub account: AccountInfo,
    pub username: Option<String>,
    pub phone: Option<String>,
    pub phone_verified: bool,
    pub discoverable_by_phone: bool,
    pub privacy: PrivacySettings,
    pub backup_enabled: bool,
    /// Two-step verification (TOTP) is required at login.
    pub totp_enabled: bool,
    /// New password logins must be approved on one of the account's devices.
    pub login_approval: bool,
    /// The password changed on another device: the local data is still locked with the
    /// old one until the user enters the new password here.
    pub local_password_outdated: bool,
    /// The account password is below the current password rules: ask to change it.
    pub password_weak: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct SafetyNumber {
    pub digits: String,
    pub qr_payload: String,
    pub verified: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ContactImportResult {
    pub total: usize,
    pub with_phone: usize,
    pub matched: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct Presence {
    pub account_id: String,
    pub online: bool,
    pub last_seen: Option<i64>,
}

// ---------------------------------------------------------------------------
// Requests
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct OutgoingAttachment {
    pub path: Option<String>,
    pub data_base64: Option<String>,
    pub file_name: String,
    pub mime: String,
    pub kind: String,
    pub duration_ms: Option<i64>,
    pub waveform: Option<Vec<f32>>,
    pub width: Option<i64>,
    pub height: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SendMessageRequest {
    pub conversation_id: String,
    pub text: Option<String>,
    /// @mentions inside `text`.
    #[serde(default)]
    pub mentions: Vec<Mention>,
    pub reply_to: Option<String>,
    #[serde(default)]
    pub attachments: Vec<OutgoingAttachment>,
    pub location: Option<Location>,
    /// Account ids of contacts to share as contact cards.
    #[serde(default)]
    pub contact_ids: Vec<String>,
    /// View-once media: exactly one image or video, no text (engine/view_once.rs).
    #[serde(default)]
    pub view_once: bool,
}

/// `Option<Option<T>>` helper: distinguishes "absent" from "null".
fn double_option<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(Some(Option::deserialize(d)?))
}

#[derive(Debug, Clone, Deserialize)]
pub struct ConversationSettingsPatch {
    pub conversation_id: String,
    #[serde(default, deserialize_with = "double_option")]
    pub muted_until: Option<Option<i64>>,
    pub archived: Option<bool>,
    pub pinned: Option<bool>,
    pub disappearing_seconds: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateGroupRequest {
    pub name: String,
    pub description: Option<String>,
    pub member_account_ids: Vec<String>,
    pub avatar_path: Option<String>,
    /// Initial group rules (disappearing timer, member list visibility, member names).
    #[serde(default)]
    pub settings: Option<GroupSettingsPatch>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpdateGroupRequest {
    pub conversation_id: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub avatar_path: Option<String>,
    pub settings: Option<GroupSettingsPatch>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SetProfileRequest {
    pub name: Option<String>,
    pub about: Option<String>,
    pub avatar_path: Option<String>,
    /// New avatar as image bytes (e.g. a camera snapshot); alternative to `avatar_path`.
    #[serde(default)]
    pub avatar_data_base64: Option<String>,
}

/// An account found in the directory (by account number or username).
#[derive(Debug, Clone, Serialize)]
pub struct FoundAccount {
    pub account_id: String,
    pub number: String,
    pub username: Option<String>,
    pub display_name: String,
    pub avatar_path: Option<String>,
    pub is_me: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PrivacyPatch {
    pub last_seen: Option<String>,
    pub read_receipts: Option<bool>,
    pub typing_indicators: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TypingEventPayload {
    pub conversation_id: String,
    pub account_id: String,
    pub sender_name: String,
    pub typing: bool,
}

/// One version of an edited message (see engine/edits.rs).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EditVersion {
    pub text: String,
    /// When this version was written (sent or edited).
    pub at: i64,
}
