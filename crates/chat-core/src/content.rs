//! End-to-end encrypted payload format (the plaintext inside Signal messages).
//!
//! Serialized as JSON, then padded (see `crypto::pad`) before encryption.

use serde::{Deserialize, Serialize};

use crate::model::{Location, SharedContact};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Content {
    /// A user-visible message (or a modification of one: edit, delete, reaction).
    Data(DataMessage),
    /// Delivery / read receipts for message ids.
    Receipt { kind: ReceiptKind, ids: Vec<String> },
    /// Typing indicator (sent as ephemeral, never stored by the server).
    Typing { typing: bool, group_id: Option<String> },
    /// Messages from our own other devices.
    Sync(SyncMessage),
    /// Sender key distribution for a group (sent pairwise).
    SenderKey { group_id: String, skdm: String },
    /// Group state change notification carrying the group key (sent pairwise).
    Group(GroupControl),
    /// Our profile (name, about, avatar) changed: contacts should refetch it.
    ProfileUpdated { profile_key: String },
    /// Asks the sender to resend a message we could not decrypt.
    ResendRequest { timestamp: i64, group_id: Option<String> },
    /// Voice call signaling (WebRTC offer / answer / ICE, ringing, hangup); see engine/calls.rs.
    Call(CallSignal),
    /// The sender deleted our direct conversation "for both": wipe everything up to `before`.
    /// Sent to our own other devices too, with `peer` naming the conversation (absent when it is
    /// sent to the peer, whose conversation is the one with the sender).
    ConversationWiped {
        before: i64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        peer: Option<String>,
    },
    /// The sender deleted the messages they sent to a group up to `before`.
    GroupMessagesWiped { group_id: String, before: i64 },
    /// A group admin cleared the group history for every member up to `before`
    /// (receivers check that the sender is an admin).
    GroupHistoryCleared { group_id: String, before: i64 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallSignal {
    pub call_id: String,
    /// "offer" | "answer" | "ice" | "ringing" | "hangup" | "busy"
    pub kind: String,
    /// Kind-specific payload (SDP, ICE candidates, hangup reason), opaque to the core.
    #[serde(default)]
    pub data: serde_json::Value,
    /// When set, only this device of the recipient handles the signal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_device: Option<u32>,    /// Group call signaling (engine/group_calls.rs); None for one-to-one calls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptKind {
    Delivered,
    Read,
    /// View-once media was opened (engine/view_once.rs); implies read.
    Viewed,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DataMessage {
    /// Globally unique message id (UUID) chosen by the sender.
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// @mentions inside `body`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mentions: Vec<crate::model::Mention>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<AttachmentPointer>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<Location>,
    /// Shared contact cards.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub contacts: Vec<SharedContact>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quote: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reaction: Option<ReactionAction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edit: Option<EditAction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delete: Option<String>,
    /// Disappearing timer (seconds) of the conversation, 0 = off.
    #[serde(default)]
    pub expire_timer: i64,
    /// True when this message only announces a new disappearing timer.
    #[serde(default, skip_serializing_if = "is_false")]
    pub timer_update: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub forwarded: bool,
    /// View-once media (one image or video, no text): see engine/view_once.rs.
    #[serde(default, skip_serializing_if = "is_false")]
    pub view_once: bool,
    /// Sender's profile key (base64), so recipients can decrypt the sender's profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_key: Option<String>,
    /// Sender's latest verified key transparency tree head (gossip, engine/keytrans.rs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kt: Option<crate::keytrans::TreeHead>,
    pub sent_at: i64,
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReactionAction {
    pub target_id: String,
    /// None removes the reaction.
    pub emoji: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditAction {
    pub target_id: String,
    pub body: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mentions: Vec<crate::model::Mention>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AttachmentPointer {
    /// Server attachment id.
    pub cdn_id: String,
    /// base64 64-byte key (AES || HMAC).
    pub key: String,
    /// base64 SHA-256 of the ciphertext.
    pub digest: String,
    /// Plaintext size.
    pub size: i64,
    pub mime: String,
    pub file_name: String,
    /// "image" | "video" | "file" | "voice"
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waveform: Option<Vec<f32>>,
    /// Tiny preview as a data: URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "s", rename_all = "snake_case")]
pub enum SyncMessage {
    /// A message we sent from another device to a direct conversation.
    Sent { destination: String, message: DataMessage },
    /// Messages we read on another device: (sender account id, message id).
    Read { entries: Vec<(String, String)> },
    /// Our vault changed: pull new records.
    VaultUpdated,
    /// Our profile changed: refetch it.
    ProfileUpdated,
    /// We opened a view-once message on another device: (sender account id, message id).
    ViewOnceOpened { sender: String, id: String },
    /// Our profile key was rotated on another device (base64; engine/profile_key.rs).
    ProfileKey { key: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupControl {
    pub group_id: String,
    /// base64 32-byte group key (encrypts the group metadata).
    pub group_key: String,
    pub revision: i64,
    /// Human-readable change for the system message.
    pub change: GroupChange,
    /// Group message timestamp for dedupe of system messages.
    pub id: String,
    /// Full member list (with roles) as the sending admin knows it, end-to-end authenticated
    /// by the pairwise Signal session. Receivers trust group membership only from these
    /// snapshots (see engine/attest.rs), never from the server's list alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub members: Option<Vec<AttestedMember>>,
    /// JoinedByLink: proof that the sender knows the invite link secret (which the server
    /// never sees), so the server cannot add members by pretending they used the link.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub join_proof: Option<String>,
}

/// One member in a membership snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttestedMember {
    pub account: String,
    pub role: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "c", rename_all = "snake_case")]
pub enum GroupChange {
    Created,
    MembersAdded { accounts: Vec<String> },
    MemberRemoved { account: String },
    Left,
    Renamed { name: String },
    AvatarChanged,
    DescriptionChanged,
    AdminChanged { account: String, admin: bool },
    SettingsChanged,
    KeyRotated,
    /// The sender joined with the group's invite link.
    JoinedByLink,
    /// An admin hands the current membership snapshot to a member (e.g. one who just joined
    /// by link); no system message.
    Snapshot,
}

/// Encrypted group metadata stored on the server.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GroupMetadata {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub avatar: Option<AvatarRef>,
    /// Secret of the group's invite link (base64url), when there is one (see engine/invites.rs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invite: Option<String>,
}

/// Reference to an encrypted avatar attachment.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AvatarRef {
    pub cdn_id: String,
    pub key: String,
    pub digest: String,
}

/// Encrypted profile stored on the server.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProfileData {
    pub name: String,
    #[serde(default)]
    pub about: String,
    #[serde(default)]
    pub avatar: Option<AvatarRef>,
}

/// Largest decrypted payload accepted (a 64K-character text with 32 attachment pointers is
/// well under it).
pub const MAX_CONTENT_BYTES: usize = 1024 * 1024;

impl Content {
    pub fn encode(&self) -> Vec<u8> {
        crate::crypto::pad(&serde_json::to_vec(self).expect("content serializes"))
    }

    /// Parses a decrypted payload; anything larger than `MAX_CONTENT_BYTES` is refused before
    /// the JSON parser sees it.
    pub fn decode(data: &[u8]) -> crate::error::Result<Self> {
        if data.len() > MAX_CONTENT_BYTES {
            return Err(crate::error::CoreError::invalid("message content is too large"));
        }
        Ok(serde_json::from_slice(crate::crypto::unpad(data)?)?)
    }
}
