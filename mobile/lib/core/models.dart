// Data types shared with the Rust core. They mirror app/src/lib/types.ts (and the serde structs
// of chat-core) 1:1; JSON field names are snake_case. Timestamps are milliseconds since the epoch.

typedef Json = Map<String, dynamic>;

String? _str(Object? v) => v?.toString();
int _int(Object? v, [int fallback = 0]) => v is num ? v.toInt() : (v is String ? int.tryParse(v) ?? fallback : fallback);
int? _intOrNull(Object? v) => v is num ? v.toInt() : (v is String ? int.tryParse(v) : null);
double _double(Object? v) => v is num ? v.toDouble() : (v is String ? double.tryParse(v) ?? 0 : 0);
bool _bool(Object? v) => v == true;
List<T> _list<T>(Object? v, T Function(Json) f) =>
    v is List ? v.whereType<Map>().map((e) => f(Json.from(e))).toList() : <T>[];
List<String> _strings(Object? v) => v is List ? v.map((e) => e.toString()).toList() : <String>[];
Json? _map(Object? v) => v is Map ? Json.from(v) : null;

class ServerInfo {
  final String url;
  final String name;
  final String version;
  final bool registrationEnabled;
  final bool phoneVerificationEnabled;
  final int maxAttachmentBytes;
  final int maxDevices;

  const ServerInfo({
    required this.url,
    required this.name,
    required this.version,
    required this.registrationEnabled,
    required this.phoneVerificationEnabled,
    required this.maxAttachmentBytes,
    required this.maxDevices,
  });

  factory ServerInfo.fromJson(Json j) => ServerInfo(
        url: _str(j['url']) ?? '',
        name: _str(j['name']) ?? '',
        version: _str(j['version']) ?? '',
        registrationEnabled: _bool(j['registration_enabled']),
        phoneVerificationEnabled: _bool(j['phone_verification_enabled']),
        maxAttachmentBytes: _int(j['max_attachment_bytes']),
        maxDevices: _int(j['max_devices']),
      );
}

class AccountInfo {
  final String accountId;

  /// 16-digit account number, without spaces.
  final String number;
  final int deviceId;
  final String serverUrl;

  const AccountInfo({required this.accountId, required this.number, required this.deviceId, required this.serverUrl});

  factory AccountInfo.fromJson(Json j) => AccountInfo(
        accountId: _str(j['account_id']) ?? '',
        number: _str(j['number']) ?? '',
        deviceId: _int(j['device_id']),
        serverUrl: _str(j['server_url']) ?? '',
      );
}

class AppState {
  /// "no_account" | "ready" | "locked" (local data waits for the account password)
  final String status;
  final AccountInfo? account;
  final String defaultServerUrl;

  /// Recovery phrase of a just-created account not yet confirmed by the user.
  final List<String>? pendingMnemonic;

  /// The local database is locked with the account password.
  final bool localProtected;

  const AppState({required this.status, this.account, required this.defaultServerUrl, this.pendingMnemonic, this.localProtected = false});

  bool get ready => status == 'ready';

  factory AppState.fromJson(Json j) => AppState(
        status: _str(j['status']) ?? 'no_account',
        account: _map(j['account']) == null ? null : AccountInfo.fromJson(_map(j['account'])!),
        defaultServerUrl: _str(j['default_server_url']) ?? '',
        pendingMnemonic: j['pending_mnemonic'] is List ? _strings(j['pending_mnemonic']) : null,
        localProtected: _bool(j['local_protected']),
      );
}

class RegisterResult {
  final AccountInfo account;

  /// 24 BIP39 words. Shown to the user exactly once.
  final List<String> mnemonic;

  const RegisterResult({required this.account, required this.mnemonic});

  factory RegisterResult.fromJson(Json j) =>
      RegisterResult(account: AccountInfo.fromJson(_map(j['account']) ?? {}), mnemonic: _strings(j['mnemonic']));
}

/// "connecting" | "online" | "offline"
typedef ConnectionState = String;

class Profile {
  final String name;
  final String about;
  final String? avatarPath;

  const Profile({required this.name, required this.about, this.avatarPath});

  factory Profile.fromJson(Json j) =>
      Profile(name: _str(j['name']) ?? '', about: _str(j['about']) ?? '', avatarPath: _str(j['avatar_path']));
}

class Contact {
  final String accountId;
  final String number;
  final String? profileName;
  final String? localName;

  /// Best name to show: local_name ?? profile_name ?? formatted number.
  final String displayName;
  final String? about;
  final String? avatarPath;
  final String? phone;
  final bool blocked;
  final bool verified;

  const Contact({
    required this.accountId,
    required this.number,
    this.profileName,
    this.localName,
    required this.displayName,
    this.about,
    this.avatarPath,
    this.phone,
    required this.blocked,
    required this.verified,
  });

  factory Contact.fromJson(Json j) => Contact(
        accountId: _str(j['account_id']) ?? '',
        number: _str(j['number']) ?? '',
        profileName: _str(j['profile_name']),
        localName: _str(j['local_name']),
        displayName: _str(j['display_name']) ?? '',
        about: _str(j['about']),
        avatarPath: _str(j['avatar_path']),
        phone: _str(j['phone']),
        blocked: _bool(j['blocked']),
        verified: _bool(j['verified']),
      );
}

/// "text" | "image" | "video" | "file" | "voice" | "location" | "system"
typedef MessageKind = String;

/// "pending" | "sent" | "delivered" | "read" | "failed"
typedef MessageStatus = String;

class MessagePreview {
  final String id;
  final MessageKind kind;
  final String? text;
  final String? senderName;
  final bool outgoing;
  final MessageStatus status;
  final int sentAt;
  final bool deleted;

  /// For system messages: the event (chat list text).
  final SystemEvent? system;

  /// View-once photo/video (chat list shows a placeholder, never a caption).
  final bool viewOnce;

  /// The media was erased by the retention period (engine/retention.rs): placeholder text.
  final bool mediaExpired;

  const MessagePreview({
    required this.id,
    required this.kind,
    this.text,
    this.senderName,
    required this.outgoing,
    required this.status,
    required this.sentAt,
    required this.deleted,
    this.system,
    this.viewOnce = false,
    this.mediaExpired = false,
  });

  factory MessagePreview.fromJson(Json j) => MessagePreview(
        id: _str(j['id']) ?? '',
        kind: _str(j['kind']) ?? 'text',
        text: _str(j['text']),
        senderName: _str(j['sender_name']),
        outgoing: _bool(j['outgoing']),
        status: _str(j['status']) ?? 'sent',
        sentAt: _int(j['sent_at']),
        deleted: _bool(j['deleted']),
        system: _map(j['system']) == null ? null : SystemEvent.fromJson(_map(j['system'])!),
        viewOnce: _bool(j['view_once']),
        mediaExpired: _bool(j['media_expired']),
      );
}

class Conversation {
  final String id;

  /// "direct" | "group"
  final String kind;
  final String title;
  final String? avatarPath;
  final String? peerAccountId;
  final String? peerNumber;
  final String? groupId;
  final int? memberCount;
  final bool isAdmin;
  final bool left;
  final MessagePreview? lastMessage;
  final int unreadCount;

  /// Unread messages that @mention us.
  final int unreadMentions;

  /// null = not muted, -1 = muted forever, otherwise ms timestamp.
  final int? mutedUntil;
  final bool archived;
  final bool pinned;
  final int disappearingSeconds;
  final bool blocked;
  final String? draft;
  final int updatedAt;

  /// false: a message request (a stranger wrote first) or a group a stranger added us to.
  /// Until accepted nothing is downloaded or acknowledged and calls do not ring.
  final bool accepted;

  /// Direct chats: the peer is in our contacts (calls with anyone else are relayed).
  final bool peerIsContact;

  const Conversation({
    required this.id,
    required this.kind,
    required this.title,
    this.avatarPath,
    this.peerAccountId,
    this.peerNumber,
    this.groupId,
    this.memberCount,
    required this.isAdmin,
    required this.left,
    this.lastMessage,
    required this.unreadCount,
    this.unreadMentions = 0,
    this.mutedUntil,
    required this.archived,
    required this.pinned,
    required this.disappearingSeconds,
    required this.blocked,
    this.draft,
    required this.updatedAt,
    this.accepted = true,
    this.peerIsContact = false,
  });

  bool get isGroup => kind == 'group';

  /// Shown as a message request / group invite (accept, block or delete).
  bool get isRequest => !accepted && !left && !blocked;

  bool isMuted([int? now]) {
    final m = mutedUntil;
    if (m == null) return false;
    if (m == -1) return true;
    return m > (now ?? DateTime.now().millisecondsSinceEpoch);
  }

  /// Timestamp used to sort the chat list.
  int get sortTime => lastMessage?.sentAt ?? updatedAt;

  factory Conversation.fromJson(Json j) => Conversation(
        id: _str(j['id']) ?? '',
        kind: _str(j['kind']) ?? 'direct',
        title: _str(j['title']) ?? '',
        avatarPath: _str(j['avatar_path']),
        peerAccountId: _str(j['peer_account_id']),
        peerNumber: _str(j['peer_number']),
        groupId: _str(j['group_id']),
        memberCount: _intOrNull(j['member_count']),
        isAdmin: _bool(j['is_admin']),
        left: _bool(j['left']),
        lastMessage: _map(j['last_message']) == null ? null : MessagePreview.fromJson(_map(j['last_message'])!),
        unreadCount: _int(j['unread_count']),
        unreadMentions: _int(j['unread_mentions']),
        mutedUntil: _intOrNull(j['muted_until']),
        archived: _bool(j['archived']),
        pinned: _bool(j['pinned']),
        disappearingSeconds: _int(j['disappearing_seconds']),
        blocked: _bool(j['blocked']),
        draft: _str(j['draft']),
        updatedAt: _int(j['updated_at']),
        accepted: j['accepted'] != false,
        peerIsContact: _bool(j['peer_is_contact']),
      );
}

class Attachment {
  final String id;
  final String mime;
  final String fileName;
  final int size;
  final int? width;
  final int? height;
  final int? durationMs;

  /// Normalized 0..1 amplitudes for voice notes (<= 64 values).
  final List<double>? waveform;

  /// Tiny blurred preview (data: URL), available before download.
  final String? thumbnail;

  /// Decrypted local file.
  final String? localPath;

  /// "pending" | "downloading" | "done" | "failed" | "expired" (erased by the retention period:
  /// only kind, type, size and, for documents, the name remain).
  final String downloadState;
  final double? progress;

  /// When the retention erased the media (ms); null while it is available.
  final int? expiredAt;

  const Attachment({
    required this.id,
    required this.mime,
    required this.fileName,
    required this.size,
    this.width,
    this.height,
    this.durationMs,
    this.waveform,
    this.thumbnail,
    this.localPath,
    required this.downloadState,
    this.progress,
    this.expiredAt,
  });

  bool get isImage => mime.startsWith('image/');
  /// Erased by the retention period: a placeholder, no download / save / forward.
  bool get isExpired => downloadState == 'expired' || expiredAt != null;
  bool get isVideo => mime.startsWith('video/');

  factory Attachment.fromJson(Json j) => Attachment(
        id: _str(j['id']) ?? '',
        mime: _str(j['mime']) ?? 'application/octet-stream',
        fileName: _str(j['file_name']) ?? '',
        size: _int(j['size']),
        width: _intOrNull(j['width']),
        height: _intOrNull(j['height']),
        durationMs: _intOrNull(j['duration_ms']),
        waveform: j['waveform'] is List ? (j['waveform'] as List).map(_double).toList() : null,
        thumbnail: _str(j['thumbnail']),
        localPath: _str(j['local_path']),
        downloadState: _str(j['download_state']) ?? 'pending',
        progress: j['progress'] is num ? (j['progress'] as num).toDouble() : null,
        expiredAt: _intOrNull(j['expired_at']),
      );
}

/// A contact card shared in a message. `name` is the person's own profile name.
class SharedContact {
  final String accountId;
  final String number;
  final String name;

  const SharedContact({required this.accountId, required this.number, required this.name});

  factory SharedContact.fromJson(Json j) =>
      SharedContact(accountId: _str(j['account_id']) ?? '', number: _str(j['number']) ?? '', name: _str(j['name']) ?? '');
}

class Location {
  final double lat;
  final double lng;
  final String? name;
  final String? address;

  const Location({required this.lat, required this.lng, this.name, this.address});

  factory Location.fromJson(Json j) =>
      Location(lat: _double(j['lat']), lng: _double(j['lng']), name: _str(j['name']), address: _str(j['address']));

  Json toJson() => {'lat': lat, 'lng': lng, 'name': name, 'address': address};
}

class Reaction {
  final String emoji;
  final String accountId;
  final String senderName;
  final bool isMe;

  const Reaction({required this.emoji, required this.accountId, required this.senderName, required this.isMe});

  factory Reaction.fromJson(Json j) => Reaction(
        emoji: _str(j['emoji']) ?? '',
        accountId: _str(j['account_id']) ?? '',
        senderName: _str(j['sender_name']) ?? '',
        isMe: _bool(j['is_me']),
      );
}

class QuotedMessage {
  final String id;
  final String senderName;
  final MessageKind kind;
  final String? text;
  final String? thumbnail;

  const QuotedMessage({required this.id, required this.senderName, required this.kind, this.text, this.thumbnail});

  factory QuotedMessage.fromJson(Json j) => QuotedMessage(
        id: _str(j['id']) ?? '',
        senderName: _str(j['sender_name']) ?? '',
        kind: _str(j['kind']) ?? 'text',
        text: _str(j['text']),
        thumbnail: _str(j['thumbnail']),
      );
}

class SystemEvent {
  /// group_created, group_member_added, group_member_removed, group_member_left, group_renamed,
  /// group_avatar_changed, group_admin_granted, group_admin_revoked, disappearing_changed,
  /// identity_changed, contact_joined
  final String type;
  final String? actorName;
  final List<String> targetNames;
  final String? value;

  const SystemEvent({required this.type, this.actorName, required this.targetNames, this.value});

  factory SystemEvent.fromJson(Json j) => SystemEvent(
        type: _str(j['type']) ?? '',
        actorName: _str(j['actor_name']),
        targetNames: _strings(j['target_names']),
        value: _str(j['value']),
      );
}

class Message {
  final String id;
  final String conversationId;
  final String senderAccountId;
  final String senderName;
  final bool outgoing;
  final MessageKind kind;
  final String? text;
  final List<Attachment> attachments;
  final Location? location;
  final List<SharedContact> contacts;
  final QuotedMessage? replyTo;
  final List<Reaction> reactions;
  final MessageStatus status;

  /// Outgoing only: why the message is still pending or failed. "no_devices": the recipient
  /// has no connected device right now (it is sent when they are back); "account_gone": the
  /// recipient account was deleted.
  final String? statusReason;
  final int sentAt;
  final int receivedAt;
  final int? editedAt;
  final bool deleted;
  final bool forwarded;
  final int? expiresAt;
  final SystemEvent? system;

  /// @mentions inside [text] (UTF-16 ranges, "@" included).
  final List<Mention> mentions;

  /// We are mentioned.
  final bool mentionsMe;

  /// Pinned in its chat (this device only) until this time (ms).
  final int? pinnedUntil;

  /// Starred (this device only, for a week).
  final bool starred;

  /// View-once media: "unopened" | "opened" | "expired"; null for ordinary messages.
  /// Such messages never carry a local path or thumbnail (see [Backend.openViewOnce]).
  final String? viewOnce;

  bool get isViewOnce => viewOnce != null;

  const Message({
    required this.id,
    required this.conversationId,
    required this.senderAccountId,
    required this.senderName,
    required this.outgoing,
    required this.kind,
    this.text,
    this.attachments = const [],
    this.location,
    this.contacts = const [],
    this.replyTo,
    this.reactions = const [],
    required this.status,
    this.statusReason,
    required this.sentAt,
    required this.receivedAt,
    this.editedAt,
    required this.deleted,
    required this.forwarded,
    this.expiresAt,
    this.system,
    this.mentions = const [],
    this.mentionsMe = false,
    this.pinnedUntil,
    this.starred = false,
    this.viewOnce,
  });

  factory Message.fromJson(Json j) => Message(
        id: _str(j['id']) ?? '',
        conversationId: _str(j['conversation_id']) ?? '',
        senderAccountId: _str(j['sender_account_id']) ?? '',
        senderName: _str(j['sender_name']) ?? '',
        outgoing: _bool(j['outgoing']),
        kind: _str(j['kind']) ?? 'text',
        text: _str(j['text']),
        attachments: _list(j['attachments'], Attachment.fromJson),
        location: _map(j['location']) == null ? null : Location.fromJson(_map(j['location'])!),
        contacts: _list(j['contacts'], SharedContact.fromJson),
        replyTo: _map(j['reply_to']) == null ? null : QuotedMessage.fromJson(_map(j['reply_to'])!),
        reactions: _list(j['reactions'], Reaction.fromJson),
        status: _str(j['status']) ?? 'sent',
        statusReason: _str(j['status_reason']),
        sentAt: _int(j['sent_at']),
        receivedAt: _int(j['received_at']),
        editedAt: _intOrNull(j['edited_at']),
        deleted: _bool(j['deleted']),
        forwarded: _bool(j['forwarded']),
        expiresAt: _intOrNull(j['expires_at']),
        system: _map(j['system']) == null ? null : SystemEvent.fromJson(_map(j['system'])!),
        mentions: _list(j['mentions'], Mention.fromJson),
        mentionsMe: _bool(j['mentions_me']),
        pinnedUntil: _intOrNull(j['pinned_until']),
        starred: _bool(j['starred']),
        viewOnce: _str(j['view_once']),
      );
}

class Mention {
  final String accountId;
  final int start;
  final int length;

  const Mention({required this.accountId, required this.start, required this.length});

  factory Mention.fromJson(Json j) => Mention(accountId: _str(j['account_id']) ?? '', start: _int(j['start']), length: _int(j['length']));

  Json toJson() => {'account_id': accountId, 'start': start, 'length': length};
}

class MessageReceipt {
  final String accountId;
  final String displayName;
  final String? avatarPath;
  final MessageStatus status;
  final int? deliveredAt;
  final int? readAt;

  const MessageReceipt({
    required this.accountId,
    required this.displayName,
    this.avatarPath,
    required this.status,
    this.deliveredAt,
    this.readAt,
  });

  factory MessageReceipt.fromJson(Json j) => MessageReceipt(
        accountId: _str(j['account_id']) ?? '',
        displayName: _str(j['display_name']) ?? '',
        avatarPath: _str(j['avatar_path']),
        status: _str(j['status']) ?? 'sent',
        deliveredAt: _intOrNull(j['delivered_at']),
        readAt: _intOrNull(j['read_at']),
      );
}

class MessageInfo {
  final Message message;
  final List<MessageReceipt> receipts;

  const MessageInfo({required this.message, required this.receipts});

  factory MessageInfo.fromJson(Json j) => MessageInfo(
        message: Message.fromJson(_map(j['message']) ?? {}),
        receipts: _list(j['receipts'], MessageReceipt.fromJson),
      );
}

/// One version of an edited message (oldest first; the last one is the current text).
class EditVersion {
  final String text;
  final int at;

  const EditVersion({required this.text, required this.at});

  factory EditVersion.fromJson(Json j) => EditVersion(text: _str(j['text']) ?? '', at: _int(j['at']));
}

class GroupSettings {
  final bool onlyAdminsSend;
  final bool onlyAdminsEdit;

  /// Disappearing timer of the whole group (admins only), 0 = off.
  final int disappearingSeconds;

  /// Members (not admins) see only how many members there are.
  final bool hideMembers;

  /// "number" | "first_name" (profile name required, only its first word is shown).
  final String memberNames;

  const GroupSettings({
    required this.onlyAdminsSend,
    required this.onlyAdminsEdit,
    this.disappearingSeconds = 0,
    this.hideMembers = false,
    this.memberNames = 'number',
  });

  bool get firstNames => memberNames == 'first_name';

  factory GroupSettings.fromJson(Json j) => GroupSettings(
        onlyAdminsSend: _bool(j['only_admins_send']),
        onlyAdminsEdit: _bool(j['only_admins_edit']),
        disappearingSeconds: _int(j['disappearing_seconds']),
        hideMembers: _bool(j['hide_members']),
        memberNames: _str(j['member_names']) ?? 'number',
      );

  Json toJson() => {
        'only_admins_send': onlyAdminsSend,
        'only_admins_edit': onlyAdminsEdit,
        'disappearing_seconds': disappearingSeconds,
        'hide_members': hideMembers,
        'member_names': memberNames,
      };

  GroupSettings copyWith({bool? onlyAdminsSend, bool? onlyAdminsEdit, bool? hideMembers, String? memberNames}) => GroupSettings(
        onlyAdminsSend: onlyAdminsSend ?? this.onlyAdminsSend,
        onlyAdminsEdit: onlyAdminsEdit ?? this.onlyAdminsEdit,
        disappearingSeconds: disappearingSeconds,
        hideMembers: hideMembers ?? this.hideMembers,
        memberNames: memberNames ?? this.memberNames,
      );
}

class GroupMember {
  final String accountId;
  final String number;
  final String displayName;
  final String? avatarPath;

  /// "admin" | "member"
  final String role;
  final bool isMe;

  /// Confirmed end to end by an admin (false: only the server lists this member).
  final bool verified;

  const GroupMember({
    required this.accountId,
    required this.number,
    required this.displayName,
    this.avatarPath,
    required this.role,
    required this.isMe,
    this.verified = true,
  });

  bool get isAdmin => role == 'admin';

  factory GroupMember.fromJson(Json j) => GroupMember(
        accountId: _str(j['account_id']) ?? '',
        number: _str(j['number']) ?? '',
        displayName: _str(j['display_name']) ?? '',
        avatarPath: _str(j['avatar_path']),
        role: _str(j['role']) ?? 'member',
        isMe: _bool(j['is_me']),
        verified: j['verified'] == null || _bool(j['verified']),
      );
}

class GroupInfo {
  final String conversationId;
  final String groupId;
  final String name;
  final String description;
  final String? avatarPath;
  /// Only ourselves when the member list is hidden from us.
  final List<GroupMember> members;
  final int memberCount;
  final bool membersHidden;

  /// "admin" | "member" | null
  final String? myRole;
  final GroupSettings settings;
  final String? createdBy;
  final int createdAt;

  /// The invite link (admins only; null when there is none).
  final String? inviteLink;

  /// Joining with the link needs an admin's approval.
  final bool inviteApproval;

  /// Pending join requests (admins only).
  final int joinRequests;

  const GroupInfo({
    required this.conversationId,
    required this.groupId,
    required this.name,
    required this.description,
    this.avatarPath,
    required this.members,
    this.memberCount = 0,
    this.membersHidden = false,
    this.myRole,
    required this.settings,
    this.createdBy,
    required this.createdAt,
    this.inviteLink,
    this.inviteApproval = false,
    this.joinRequests = 0,
  });

  factory GroupInfo.fromJson(Json j) => GroupInfo(
        conversationId: _str(j['conversation_id']) ?? '',
        groupId: _str(j['group_id']) ?? '',
        name: _str(j['name']) ?? '',
        description: _str(j['description']) ?? '',
        avatarPath: _str(j['avatar_path']),
        members: _list(j['members'], GroupMember.fromJson),
        memberCount: _int(j['member_count']),
        membersHidden: _bool(j['members_hidden']),
        myRole: _str(j['my_role']),
        settings: GroupSettings.fromJson(_map(j['settings']) ?? {}),
        createdBy: _str(j['created_by']),
        createdAt: _int(j['created_at']),
        inviteLink: _str(j['invite_link']),
        inviteApproval: _bool(j['invite_approval']),
        joinRequests: _int(j['join_requests']),
      );
}

/// What an invite link leads to, before joining.
class InvitePreview {
  final String groupId;
  final String name;
  final String description;
  final int memberCount;
  final bool approval;
  final bool isMember;
  final bool requested;

  /// An admin removed us (or rejected our request): joining by link is refused.
  final bool banned;
  final String? conversationId;

  const InvitePreview({
    required this.groupId,
    required this.name,
    required this.description,
    required this.memberCount,
    required this.approval,
    required this.isMember,
    required this.requested,
    this.banned = false,
    this.conversationId,
  });

  factory InvitePreview.fromJson(Json j) => InvitePreview(
        groupId: _str(j['group_id']) ?? '',
        name: _str(j['name']) ?? '',
        description: _str(j['description']) ?? '',
        memberCount: _int(j['member_count']),
        approval: _bool(j['approval']),
        isMember: _bool(j['is_member']),
        requested: _bool(j['requested']),
        banned: _bool(j['banned']),
        conversationId: _str(j['conversation_id']),
      );
}

class JoinResult {
  /// "joined" | "requested" | "member"
  final String status;
  final String? conversationId;
  final String groupId;

  const JoinResult({required this.status, this.conversationId, required this.groupId});

  factory JoinResult.fromJson(Json j) =>
      JoinResult(status: _str(j['status']) ?? '', conversationId: _str(j['conversation_id']), groupId: _str(j['group_id']) ?? '');
}

class JoinRequest {
  final String accountId;
  final String name;
  final String? avatarPath;
  final int requestedAt;

  const JoinRequest({required this.accountId, required this.name, this.avatarPath, required this.requestedAt});

  factory JoinRequest.fromJson(Json j) => JoinRequest(
        accountId: _str(j['account_id']) ?? '',
        name: _str(j['name']) ?? '',
        avatarPath: _str(j['avatar_path']),
        requestedAt: _int(j['requested_at']),
      );
}

/// An account an admin removed (or whose join request was rejected): it can't rejoin by link.
class GroupBan {
  final String accountId;
  final String displayName;
  final String? avatarPath;
  final String bannedBy;
  final int bannedAt;

  const GroupBan({required this.accountId, required this.displayName, this.avatarPath, required this.bannedBy, required this.bannedAt});

  factory GroupBan.fromJson(Json j) => GroupBan(
        accountId: _str(j['account_id']) ?? '',
        displayName: _str(j['display_name']) ?? '',
        avatarPath: _str(j['avatar_path']),
        bannedBy: _str(j['banned_by']) ?? '',
        bannedAt: _int(j['banned_at']),
      );
}

class DeviceInfo {
  final int deviceId;
  final String name;

  /// "desktop" | "android" | "ios" | "web" | "cli"
  final String platform;
  final int createdAt;
  final int lastSeenAt;
  final bool current;

  const DeviceInfo({
    required this.deviceId,
    required this.name,
    required this.platform,
    required this.createdAt,
    required this.lastSeenAt,
    required this.current,
  });

  factory DeviceInfo.fromJson(Json j) => DeviceInfo(
        deviceId: _int(j['device_id']),
        name: _str(j['name']) ?? '',
        platform: _str(j['platform']) ?? 'desktop',
        createdAt: _int(j['created_at']),
        lastSeenAt: _int(j['last_seen_at']),
        current: _bool(j['current']),
      );
}

/// An account saved on this phone (local profile), as listed on the start screen. Hidden
/// profiles are never listed (opened by their full number).
class ProfileCard {
  final String id;
  final String? label;
  final String? last4;
  final String? serverHost;
  final int lastUsed;

  /// Opened with its password since the app started: switching to it needs no password.
  final bool unlocked;

  /// The account in use now.
  final bool active;

  const ProfileCard({required this.id, this.label, this.last4, this.serverHost, this.lastUsed = 0, this.unlocked = false, this.active = false});

  factory ProfileCard.fromJson(Json j) => ProfileCard(
        id: _str(j['id']) ?? '',
        label: _str(j['label']),
        last4: _str(j['last4']),
        serverHost: _str(j['server_host']),
        lastUsed: _int(j['last_used']),
        unlocked: j['unlocked'] == true,
        active: j['active'] == true,
      );
}

/// The active profile: whether it is saved on this phone yet and shown on the start screen.
class CurrentProfile {
  final bool registered;
  final bool visible;
  const CurrentProfile({required this.registered, required this.visible});

  factory CurrentProfile.fromJson(Json j) => CurrentProfile(registered: _bool(j['registered']), visible: j['visible'] != false);
}

class PrivacySettings {
  /// "everyone" | "nobody" (server-enforced)
  final String lastSeen;
  final bool readReceipts;
  final bool typingIndicators;

  const PrivacySettings({required this.lastSeen, required this.readReceipts, required this.typingIndicators});

  factory PrivacySettings.fromJson(Json j) => PrivacySettings(
        lastSeen: _str(j['last_seen']) ?? 'everyone',
        readReceipts: j['read_receipts'] != false,
        typingIndicators: j['typing_indicators'] != false,
      );
}

class AccountDetails {
  final AccountInfo account;

  /// Public @username (lower case, without "@"), if set.
  final String? username;
  final String? phone;
  final bool phoneVerified;
  final bool discoverableByPhone;
  final PrivacySettings privacy;
  final bool backupEnabled;

  /// Password changed on another device; the local lock still uses the old one.
  final bool localPasswordOutdated;

  /// The password is below the current rules: ask to change it.
  final bool passwordWeak;

  /// New password logins must be approved on one of the account's devices.
  final bool loginApproval;

  const AccountDetails({
    required this.account,
    this.username,
    this.phone,
    required this.phoneVerified,
    required this.discoverableByPhone,
    required this.privacy,
    required this.backupEnabled,
    this.localPasswordOutdated = false,
    this.passwordWeak = false,
    this.loginApproval = false,
  });

  factory AccountDetails.fromJson(Json j) => AccountDetails(
        account: AccountInfo.fromJson(_map(j['account']) ?? {}),
        username: _str(j['username']),
        phone: _str(j['phone']),
        phoneVerified: _bool(j['phone_verified']),
        discoverableByPhone: _bool(j['discoverable_by_phone']),
        privacy: PrivacySettings.fromJson(_map(j['privacy']) ?? {}),
        backupEnabled: _bool(j['backup_enabled']),
        localPasswordOutdated: _bool(j['local_password_outdated']),
        passwordWeak: _bool(j['password_weak']),
        loginApproval: _bool(j['login_approval']),
      );

  AccountDetails copyWith({
    PrivacySettings? privacy,
    bool? backupEnabled,
    String? username,
    bool clearUsername = false,
    bool? localPasswordOutdated,
    bool? loginApproval,
  }) =>
      AccountDetails(
        account: account,
        username: clearUsername ? null : (username ?? this.username),
        phone: phone,
        phoneVerified: phoneVerified,
        discoverableByPhone: discoverableByPhone,
        privacy: privacy ?? this.privacy,
        backupEnabled: backupEnabled ?? this.backupEnabled,
        localPasswordOutdated: localPasswordOutdated ?? this.localPasswordOutdated,
        passwordWeak: passwordWeak,
        loginApproval: loginApproval ?? this.loginApproval,
      );
}

/// A device that joined the account after this one (shown until dismissed).
class NewDeviceAlert {
  final int deviceId;
  final String name;
  final String platform;
  final int createdAt;

  const NewDeviceAlert({required this.deviceId, required this.name, required this.platform, required this.createdAt});

  factory NewDeviceAlert.fromJson(Json j) => NewDeviceAlert(
        deviceId: _int(j['device_id']),
        name: _str(j['name']) ?? '',
        platform: _str(j['platform']) ?? '',
        createdAt: _int(j['created_at']),
      );
}

/// A password login waiting for approval by one of our devices (expires after 10 minutes).
class LoginRequestInfo {
  final String id;
  final String name;
  final String platform;
  final int createdAt;
  final int expiresAt;

  const LoginRequestInfo({required this.id, required this.name, required this.platform, required this.createdAt, required this.expiresAt});

  factory LoginRequestInfo.fromJson(Json j) => LoginRequestInfo(
        id: _str(j['id']) ?? '',
        name: _str(j['name']) ?? '',
        platform: _str(j['platform']) ?? '',
        createdAt: _int(j['created_at']),
        expiresAt: _int(j['expires_at']),
      );
}

/// Shareable username link (`chatapp://u?s=<server>#<key+handle>`), without the account number.
class UsernameLink {
  final String link;
  final String username;

  const UsernameLink({required this.link, required this.username});

  factory UsernameLink.fromJson(Json j) => UsernameLink(link: _str(j['link']) ?? '', username: _str(j['username']) ?? '');
}

/// An account found in the directory (by account number or username).
class FoundAccount {
  final String accountId;
  final String number;
  final String? username;
  final String displayName;
  final String? avatarPath;
  final bool isMe;

  const FoundAccount({
    required this.accountId,
    required this.number,
    this.username,
    required this.displayName,
    this.avatarPath,
    required this.isMe,
  });

  factory FoundAccount.fromJson(Json j) => FoundAccount(
        accountId: _str(j['account_id']) ?? '',
        number: _str(j['number']) ?? '',
        username: _str(j['username']),
        displayName: _str(j['display_name']) ?? '',
        avatarPath: _str(j['avatar_path']),
        isMe: _bool(j['is_me']),
      );
}

class SafetyNumber {
  /// 60 digits in 12 groups of 5, separated by spaces.
  final String digits;

  /// Base64 scannable fingerprint (rendered as a QR code).
  final String qrPayload;
  final bool verified;

  const SafetyNumber({required this.digits, required this.qrPayload, required this.verified});

  List<String> get groups => digits.split(RegExp(r'\s+')).where((g) => g.isNotEmpty).toList();

  factory SafetyNumber.fromJson(Json j) => SafetyNumber(
        digits: _str(j['digits']) ?? '',
        qrPayload: _str(j['qr_payload']) ?? '',
        verified: _bool(j['verified']),
      );
}

class ContactImportResult {
  final int total;
  final int withPhone;
  final int matched;

  const ContactImportResult({required this.total, required this.withPhone, required this.matched});

  factory ContactImportResult.fromJson(Json j) =>
      ContactImportResult(total: _int(j['total']), withPhone: _int(j['with_phone']), matched: _int(j['matched']));
}

class Presence {
  final String accountId;
  final bool online;
  final int? lastSeen;

  const Presence({required this.accountId, required this.online, this.lastSeen});

  factory Presence.fromJson(Json j) =>
      Presence(accountId: _str(j['account_id']) ?? '', online: _bool(j['online']), lastSeen: _intOrNull(j['last_seen']));
}

// ---------------------------------------------------------------------------
// Requests
// ---------------------------------------------------------------------------

class OutgoingAttachment {
  /// Either a local file path ...
  final String? path;

  /// ... or inline bytes (voice recordings).
  final String? dataBase64;
  final String fileName;
  final String mime;

  /// "image" | "video" | "file" | "voice"
  final String kind;
  final int? durationMs;
  final List<double>? waveform;
  final int? width;
  final int? height;

  const OutgoingAttachment({
    this.path,
    this.dataBase64,
    required this.fileName,
    required this.mime,
    required this.kind,
    this.durationMs,
    this.waveform,
    this.width,
    this.height,
  });

  Json toJson() => {
        'path': ?path,
        'data_base64': ?dataBase64,
        'file_name': fileName,
        'mime': mime,
        'kind': kind,
        'duration_ms': ?durationMs,
        'waveform': ?waveform,
        'width': ?width,
        'height': ?height,
      };
}

class ConversationSettingsPatch {
  /// Set [setMuted] to write [mutedUntil] (which may be null to unmute).
  final bool setMuted;
  final int? mutedUntil;
  final bool? archived;
  final bool? pinned;
  final int? disappearingSeconds;

  const ConversationSettingsPatch({
    this.setMuted = false,
    this.mutedUntil,
    this.archived,
    this.pinned,
    this.disappearingSeconds,
  });

  Json toJson() => {
        if (setMuted) 'muted_until': mutedUntil,
        'archived': ?archived,
        'pinned': ?pinned,
        'disappearing_seconds': ?disappearingSeconds,
      };
}

// ---------------------------------------------------------------------------
// Events (Rust -> UI)
// ---------------------------------------------------------------------------

class MessageEvent {
  final String conversationId;
  final Message message;

  /// true only the first time an incoming message is stored (use for notifications).
  final bool isNewIncoming;

  const MessageEvent({required this.conversationId, required this.message, required this.isNewIncoming});

  factory MessageEvent.fromJson(Json j) => MessageEvent(
        conversationId: _str(j['conversation_id']) ?? '',
        message: Message.fromJson(_map(j['message']) ?? {}),
        isNewIncoming: _bool(j['is_new_incoming']),
      );
}

class MessageDeletedEvent {
  final String conversationId;
  final String messageId;

  const MessageDeletedEvent({required this.conversationId, required this.messageId});

  factory MessageDeletedEvent.fromJson(Json j) =>
      MessageDeletedEvent(conversationId: _str(j['conversation_id']) ?? '', messageId: _str(j['message_id']) ?? '');
}

class TypingEvent {
  final String conversationId;
  final String accountId;
  final String senderName;
  final bool typing;

  const TypingEvent({required this.conversationId, required this.accountId, required this.senderName, required this.typing});

  factory TypingEvent.fromJson(Json j) => TypingEvent(
        conversationId: _str(j['conversation_id']) ?? '',
        accountId: _str(j['account_id']) ?? '',
        senderName: _str(j['sender_name']) ?? '',
        typing: _bool(j['typing']),
      );
}

class LoggedOutEvent {
  /// "device_removed" | "account_deleted" | "logout"
  final String reason;

  const LoggedOutEvent({required this.reason});

  factory LoggedOutEvent.fromJson(Json j) => LoggedOutEvent(reason: _str(j['reason']) ?? 'logout');
}

class HistorySyncEvent {
  final bool done;
  final int records;

  const HistorySyncEvent({required this.done, required this.records});

  factory HistorySyncEvent.fromJson(Json j) => HistorySyncEvent(done: _bool(j['done']), records: _int(j['records']));
}

/// Key transparency: an inconsistency in the server's public key log (shown until dismissed).
/// kind: split_view | fork | rollback | bad_signature | key_mismatch | own_key_mismatch |
/// log_mismatch | log_key_changed (crates/chat-core/src/engine/keytrans.rs).
class KtAlert {
  final String kind;
  final String? accountId;
  final String detail;
  final int at;

  const KtAlert({required this.kind, this.accountId, required this.detail, required this.at});

  factory KtAlert.fromJson(Json j) =>
      KtAlert(kind: _str(j['kind']) ?? '', accountId: _str(j['account_id']), detail: _str(j['detail']) ?? '', at: _int(j['at']));
}

/// Key transparency state of this account and of the log. Statuses: verified | pending |
/// alert | unavailable.
class KtOverview {
  final bool enabled;
  final int? treeSize;
  final int? checkedAt;
  final String ownStatus;
  final int monitored;
  final List<KtAlert> alerts;

  const KtOverview({required this.enabled, this.treeSize, this.checkedAt, required this.ownStatus, required this.monitored, required this.alerts});

  factory KtOverview.fromJson(Json j) => KtOverview(
        enabled: _bool(j['enabled']),
        treeSize: _intOrNull(j['tree_size']),
        checkedAt: _intOrNull(j['checked_at']),
        ownStatus: _str(j['own_status']) ?? 'unavailable',
        monitored: _int(j['monitored']),
        alerts: (j['alerts'] is List ? (j['alerts'] as List) : const []).whereType<Map>().map((m) => KtAlert.fromJson(Json.from(m))).toList(),
      );
}

/// Key transparency state of one contact's security key.
class KtContactStatus {
  final String accountId;
  final String status;
  final int? treeSize;
  final int? checkedAt;

  const KtContactStatus({required this.accountId, required this.status, this.treeSize, this.checkedAt});

  factory KtContactStatus.fromJson(Json j) => KtContactStatus(
        accountId: _str(j['account_id']) ?? '',
        status: _str(j['status']) ?? 'unavailable',
        treeSize: _intOrNull(j['tree_size']),
        checkedAt: _intOrNull(j['checked_at']),
      );
}
