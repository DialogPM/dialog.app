// Typed wrappers around the Rust core commands (through flutter_rust_bridge). The command
// names, request objects and results are exactly those of the desktop app
// (app/src/lib/backend.ts): every command takes one JSON request object with snake_case fields,
// or "null" when it has none.

import 'dart:async';
import 'dart:convert';
import 'dart:typed_data';

import 'models.dart';

/// Sends a command to the core: `(cmd, reqJson) -> resultJson`. Errors are thrown with a
/// message of the form `<code>: <message>`.
typedef Invoker = Future<String> Function(String cmd, String reqJson);

/// Error returned by a core command.
class CoreError implements Exception {
  final String code;
  final String message;

  const CoreError(this.code, this.message);

  @override
  String toString() => '$code: $message';
}

final _codeRe = RegExp(r'^[a-z_]+$');

/// Parses `<code>: <message>` errors thrown by commands (same rules as parseError in backend.ts).
CoreError parseError(Object err) {
  if (err is CoreError) return err;
  var s = err.toString();
  // flutter_rust_bridge wraps anyhow errors: "AnyhowException(<code>: <message>)".
  final wrapped = RegExp(r'^\w*Exception\((.*)\)$', dotAll: true).firstMatch(s);
  if (wrapped != null) s = wrapped.group(1)!;
  final idx = s.indexOf(':');
  if (idx > 0 && _codeRe.hasMatch(s.substring(0, idx))) {
    return CoreError(s.substring(0, idx), s.substring(idx + 1).trim());
  }
  return CoreError('internal', s);
}

/// Do not disturb "until I turn it off" (i64::MAX in the core, engine/dnd.rs).
const int kDndForever = 9223372036854775807;

/// `seconds` of the default disappearing timer payload (engine/default_timer.rs), 0 = off.
int _defaultTimer(Json j) => j['seconds'] is num ? (j['seconds'] as num).toInt() : 0;

int _dndUntil(Json j) => j['until'] is num ? (j['until'] as num).toInt() : 0;

/// Result of a media check (engine/retention.rs): files removed and when (ms).
class MediaCheck {
  final int removed;
  final int checkedAt;

  const MediaCheck({required this.removed, required this.checkedAt});

  factory MediaCheck.fromJson(Json j) => MediaCheck(
        removed: j['removed'] is num ? (j['removed'] as num).toInt() : 0,
        checkedAt: j['checked_at'] is num ? (j['checked_at'] as num).toInt() : 0,
      );
}

/// Media retention period (engine/retention.rs): photos, videos, voice messages and files are
/// deleted after 1, 7, 14 or 30 days (30 is the default and the longest allowed), the same on
/// all our devices.
class MediaRetention {
  final int days;
  final List<int> options;
  final int maxDays;
  final MediaCheck? lastCheck;

  const MediaRetention({this.days = 30, this.options = const [1, 7, 14, 30], this.maxDays = 30, this.lastCheck});

  static const defaults = MediaRetention();

  factory MediaRetention.fromJson(Json j) => MediaRetention(
        days: j['days'] is num ? (j['days'] as num).toInt() : 30,
        options: j['options'] is List ? (j['options'] as List).whereType<num>().map((n) => n.toInt()).toList() : const [1, 7, 14, 30],
        maxDays: j['max_days'] is num ? (j['max_days'] as num).toInt() : 30,
        lastCheck: j['last_check'] is Map ? MediaCheck.fromJson((j['last_check'] as Map).cast<String, dynamic>()) : null,
      );

  MediaRetention withCheck(MediaCheck c) => MediaRetention(days: days, options: options, maxDays: maxDays, lastCheck: c);
}

/// Call privacy settings (engine/call_settings.rs), the same on all our devices.
class CallSettings {
  /// Calls may ring here and be placed from here (voice, video and group calls).
  final bool allowCalls;
  /// Every call goes through the TURN relay only, so no peer learns our IP address.
  final bool relayOnly;

  const CallSettings({this.allowCalls = true, this.relayOnly = false});

  static const defaults = CallSettings();

  factory CallSettings.fromJson(Json j) => CallSettings(
        allowCalls: j['allow_calls'] is bool ? j['allow_calls'] as bool : true,
        relayOnly: j['relay_only'] is bool ? j['relay_only'] as bool : false,
      );

  @override
  bool operator ==(Object other) => other is CallSettings && other.allowCalls == allowCalls && other.relayOnly == relayOnly;

  @override
  int get hashCode => Object.hash(allowCalls, relayOnly);
}

/// One event emitted by the core: `{"event": name, "payload": ...}`.
class CoreEvent {
  final String name;
  final Object? payload;

  const CoreEvent(this.name, this.payload);

  Json get json => payload is Map ? Json.from(payload as Map) : <String, dynamic>{};

  static CoreEvent? tryParse(String raw) {
    try {
      final v = jsonDecode(raw);
      if (v is Map && v['event'] is String) return CoreEvent(v['event'] as String, v['payload']);
    } catch (_) {}
    return null;
  }
}

/// Event names emitted by the core.
abstract final class Events {
  static const conversationUpdated = 'conversation_updated';
  static const conversationRemoved = 'conversation_removed';
  static const message = 'message';
  static const messageDeleted = 'message_deleted';
  static const typing = 'typing';
  static const presence = 'presence';
  static const connection = 'connection';
  static const contactUpdated = 'contact_updated';
  static const profileUpdated = 'profile_updated';
  static const devicesChanged = 'devices_changed';
  static const groupUpdated = 'group_updated';
  static const historySync = 'history_sync';
  static const loggedOut = 'logged_out';
  static const conversationsReset = 'conversations_reset';
  static const pinsChanged = 'pins_changed';
  static const callSignal = 'call_signal';
  static const groupCall = 'group_call';
  static const groupCallSignal = 'group_call_signal';
  static const deviceAlerts = 'device_alerts';
  static const loginRequests = 'login_requests';
  static const ktAlerts = 'kt_alerts';
  static const dndChanged = 'dnd_changed';
  static const callSettingsChanged = 'call_settings_changed';
  static const mediaRetentionChanged = 'media_retention_changed';
  static const mediaCheck = 'media_check';
  static const defaultDisappearingChanged = 'default_disappearing_changed';
}

/// Signaling message of a call (see crates/chat-core/src/engine/calls.rs).
class CallSignalEvent {
  final String accountId;
  final int deviceId;
  /// Direct conversation with the caller (null for signals from our own devices).
  final String? conversationId;
  final String callId;
  final String kind;
  final Json? data;
  /// Sent by one of our own devices (answered / declined there).
  final bool fromMe;
  /// Media must go through the TURN relay only (caller not accepted: hide our IP address).
  final bool relay;

  const CallSignalEvent({
    required this.accountId,
    required this.deviceId,
    this.conversationId,
    required this.callId,
    required this.kind,
    this.data,
    required this.fromMe,
    this.relay = false,
  });

  factory CallSignalEvent.fromJson(Json j) => CallSignalEvent(
        accountId: '${j['account_id']}',
        deviceId: (j['device_id'] as num?)?.toInt() ?? 0,
        conversationId: j['conversation_id'] as String?,
        callId: '${j['call_id']}',
        kind: '${j['kind']}',
        data: j['data'] is Map ? Json.from(j['data'] as Map) : null,
        fromMe: j['from_me'] == true,
        relay: j['relay'] == true,
      );
}

/// One device in a group call.
class GroupCallParticipant {
  final String accountId;
  final int deviceId;
  /// This very device.
  final bool me;
  /// Not a contact: the pair goes through the TURN relay only.
  final bool relay;

  const GroupCallParticipant({required this.accountId, required this.deviceId, this.me = false, this.relay = false});

  String get key => '$accountId:$deviceId';

  factory GroupCallParticipant.fromJson(Json j) => GroupCallParticipant(
        accountId: '${j['account_id']}',
        deviceId: (j['device_id'] as num?)?.toInt() ?? 0,
        me: j['me'] == true,
        relay: j['relay'] == true,
      );
}

/// State of a group call (see crates/chat-core/src/engine/group_calls.rs).
class GroupCallInfo {
  final String groupId;
  final String conversationId;
  final String callId;
  final String startedBy;
  final int startedAt;
  final bool video;
  final List<GroupCallParticipant> participants;
  /// This device is in the call.
  final bool joined;
  /// This device should ring.
  final bool ringing;
  /// The call is over.
  final bool ended;
  final int maxParticipants;

  const GroupCallInfo({
    required this.groupId,
    required this.conversationId,
    required this.callId,
    required this.startedBy,
    required this.startedAt,
    required this.video,
    required this.participants,
    required this.joined,
    required this.ringing,
    required this.ended,
    required this.maxParticipants,
  });

  factory GroupCallInfo.fromJson(Json j) => GroupCallInfo(
        groupId: '${j['group_id']}',
        conversationId: '${j['conversation_id']}',
        callId: '${j['call_id']}',
        startedBy: '${j['started_by']}',
        startedAt: (j['started_at'] as num?)?.toInt() ?? 0,
        video: j['video'] == true,
        participants: j['participants'] is List
            ? (j['participants'] as List).whereType<Map>().map((p) => GroupCallParticipant.fromJson(Json.from(p))).toList()
            : const [],
        joined: j['joined'] == true,
        ringing: j['ringing'] == true,
        ended: j['ended'] == true,
        maxParticipants: (j['max_participants'] as num?)?.toInt() ?? 8,
      );
}

/// WebRTC signaling of one pair in a group call.
class GroupCallSignalEvent {
  final String groupId;
  final String conversationId;
  final String callId;
  final String accountId;
  final int deviceId;
  /// "offer" | "answer" | "ice" | "hangup" | "media"
  final String kind;
  final Json? data;
  final bool relay;

  const GroupCallSignalEvent({
    required this.groupId,
    required this.conversationId,
    required this.callId,
    required this.accountId,
    required this.deviceId,
    required this.kind,
    this.data,
    this.relay = false,
  });

  factory GroupCallSignalEvent.fromJson(Json j) => GroupCallSignalEvent(
        groupId: '${j['group_id']}',
        conversationId: '${j['conversation_id']}',
        callId: '${j['call_id']}',
        accountId: '${j['account_id']}',
        deviceId: (j['device_id'] as num?)?.toInt() ?? 0,
        kind: '${j['kind']}',
        data: j['data'] is Map ? Json.from(j['data'] as Map) : null,
        relay: j['relay'] == true,
      );
}

class Backend {
  Backend(this._invoke, Stream<String> rawEvents) {
    _sub = rawEvents.listen((raw) {
      final e = CoreEvent.tryParse(raw);
      if (e != null) _events.add(e);
    });
  }

  final Invoker _invoke;
  final _events = StreamController<CoreEvent>.broadcast();
  late final StreamSubscription<String> _sub;

  /// All core events.
  Stream<CoreEvent> get events => _events.stream;

  Stream<T> _on<T>(String name, T Function(CoreEvent e) parse) =>
      _events.stream.where((e) => e.name == name).map(parse);

  // --- typed event streams ----------------------------------------------------
  Stream<Conversation> get onConversationUpdated => _on(Events.conversationUpdated, (e) => Conversation.fromJson(e.json));
  Stream<String> get onConversationRemoved => _on(Events.conversationRemoved, (e) => '${e.json['conversation_id']}');
  Stream<MessageEvent> get onMessage => _on(Events.message, (e) => MessageEvent.fromJson(e.json));
  Stream<MessageDeletedEvent> get onMessageDeleted => _on(Events.messageDeleted, (e) => MessageDeletedEvent.fromJson(e.json));
  Stream<TypingEvent> get onTyping => _on(Events.typing, (e) => TypingEvent.fromJson(e.json));
  Stream<Presence> get onPresence => _on(Events.presence, (e) => Presence.fromJson(e.json));
  Stream<String> get onConnection => _on(Events.connection, (e) => '${e.payload}');
  Stream<Contact> get onContactUpdated => _on(Events.contactUpdated, (e) => Contact.fromJson(e.json));
  Stream<Profile> get onProfileUpdated => _on(Events.profileUpdated, (e) => Profile.fromJson(e.json));
  Stream<void> get onDevicesChanged => _on(Events.devicesChanged, (e) {});
  /// Current new-device alerts (empty after dismissing).
  Stream<List<NewDeviceAlert>> get onDeviceAlerts => _on(Events.deviceAlerts, (e) => _jsonList(e.payload).map(NewDeviceAlert.fromJson).toList());
  /// Current logins waiting for approval.
  Stream<List<LoginRequestInfo>> get onLoginRequests =>
      _on(Events.loginRequests, (e) => _jsonList(e.payload).map(LoginRequestInfo.fromJson).toList());
  /// Current key transparency alerts (empty after dismissing).
  Stream<List<KtAlert>> get onKtAlerts => _on(Events.ktAlerts, (e) => _jsonList(e.payload).map(KtAlert.fromJson).toList());
  Stream<GroupInfo> get onGroupUpdated => _on(Events.groupUpdated, (e) => GroupInfo.fromJson(e.json));
  Stream<HistorySyncEvent> get onHistorySync => _on(Events.historySync, (e) => HistorySyncEvent.fromJson(e.json));
  /// The conversation list was replaced wholesale (temporary mode toggled).
  Stream<void> get onConversationsReset => _on(Events.conversationsReset, (e) {});

  /// The pinned messages of a conversation changed (payload: its id).
  Stream<String> get onPinsChanged => _on(Events.pinsChanged, (e) => '${e.json['conversation_id']}');
  Stream<CallSignalEvent> get onCallSignal => _on(Events.callSignal, (e) => CallSignalEvent.fromJson(e.json));
  /// State of a group call (also its end: `ended`).
  Stream<GroupCallInfo> get onGroupCall => _on(Events.groupCall, (e) => GroupCallInfo.fromJson(e.json));
  Stream<GroupCallSignalEvent> get onGroupCallSignal => _on(Events.groupCallSignal, (e) => GroupCallSignalEvent.fromJson(e.json));
  /// Do not disturb changed (here or synced from another device): its end, 0 = off.
  Stream<int> get onDndChanged => _on(Events.dndChanged, (e) => _dndUntil(e.json));
  /// Call privacy settings changed (here or synced from another device).
  Stream<CallSettings> get onCallSettingsChanged => _on(Events.callSettingsChanged, (e) => CallSettings.fromJson(e.json));
  /// Media retention period changed (here or synced from another device).
  Stream<MediaRetention> get onMediaRetentionChanged => _on(Events.mediaRetentionChanged, (e) => MediaRetention.fromJson(e.json));
  /// The default disappearing timer changed (here or synced from another device): seconds, 0 = off.
  Stream<int> get onDefaultDisappearingChanged => _on(Events.defaultDisappearingChanged, (e) => _defaultTimer(e.json));
  /// A media check ran in the core (start, after each retention pass, on demand).
  Stream<MediaCheck> get onMediaCheck => _on(Events.mediaCheck, (e) => MediaCheck.fromJson(e.json));
  Stream<LoggedOutEvent> get onLoggedOut => _on(Events.loggedOut, (e) => LoggedOutEvent.fromJson(e.json));

  Future<void> dispose() async {
    await _sub.cancel();
    await _events.close();
  }

  /// Runs [cmd] with [req] (encoded as JSON, `null` for commands without a request) and
  /// returns the decoded result. Throws [CoreError].
  Future<dynamic> call(String cmd, [Object? req]) async {
    String out;
    try {
      out = await _invoke(cmd, jsonEncode(req));
    } catch (e) {
      throw parseError(e);
    }
    if (out.isEmpty) return null;
    return jsonDecode(out);
  }

  static List<Json> _jsonList(Object? v) => v is List ? v.whereType<Map>().map(Json.from).toList() : const [];

  Future<Json> _obj(String cmd, [Object? req]) async => Json.from((await call(cmd, req)) as Map);
  Future<List<Json>> _list(String cmd, [Object? req]) async =>
      ((await call(cmd, req)) as List).whereType<Map>().map(Json.from).toList();

  // --- app / onboarding -------------------------------------------------------
  Future<AppState> getState() async => AppState.fromJson(await _obj('get_state'));
  Future<ServerInfo> checkServer(String url) async => ServerInfo.fromJson(await _obj('check_server', {'url': url}));
  Future<RegisterResult> register({required String serverUrl, required String password, required String deviceName}) async =>
      RegisterResult.fromJson(await _obj('register', {'server_url': serverUrl, 'password': password, 'device_name': deviceName}));
  Future<AccountInfo> login({
    required String serverUrl,
    required String number,
    required String password,
    required String deviceName,
    String? totp,
  }) async =>
      AccountInfo.fromJson(await _obj('login', {
        'server_url': serverUrl,
        'number': number,
        'password': password,
        'device_name': deviceName,
        'totp': ?totp,
      }));
  Future<AccountInfo> recover({
    required String serverUrl,
    required String number,
    required List<String> mnemonic,
    required String newPassword,
    required String deviceName,
  }) async =>
      AccountInfo.fromJson(await _obj('recover', {
        'server_url': serverUrl,
        'number': number,
        'mnemonic': mnemonic,
        'new_password': newPassword,
        'device_name': deviceName,
      }));

  // --- calls ------------------------------------------------------------------
  /// Sends a call signal end-to-end encrypted to [accountId] (or to our own other devices).
  Future<void> callSignal({
    required String accountId,
    required String callId,
    required String kind,
    Object? data,
    int? targetDevice,
    bool toOwnDevices = false,
  }) =>
      call('call_signal', {
        'account_id': accountId,
        'call_id': callId,
        'kind': kind,
        'data': data,
        'target_device': targetDevice,
        'to_own_devices': toOwnDevices,
      });

  // --- group calls (full mesh; see crates/chat-core/src/engine/group_calls.rs) ---------
  /// Starts a call in a group conversation and rings the members (joins the running one).
  Future<GroupCallInfo> groupCallStart(String conversationId, {bool video = false}) async =>
      GroupCallInfo.fromJson(Json.from(await call('group_call_start', {'conversation_id': conversationId, 'video': video}) as Map));

  Future<GroupCallInfo> groupCallJoin(String conversationId, String callId, {bool video = false}) async => GroupCallInfo.fromJson(
      Json.from(await call('group_call_join', {'conversation_id': conversationId, 'call_id': callId, 'video': video}) as Map));

  Future<void> groupCallLeave(String conversationId) => call('group_call_leave', {'conversation_id': conversationId});

  /// Stops ringing here and on our other devices.
  Future<void> groupCallDecline(String conversationId) => call('group_call_decline', {'conversation_id': conversationId});

  /// Still in the call (every few seconds); without it the core leaves after a minute.
  Future<void> groupCallHeartbeat(String conversationId) => call('group_call_heartbeat', {'conversation_id': conversationId});

  /// The call in progress in a group conversation, if any.
  Future<GroupCallInfo?> groupCallState(String conversationId) async {
    final r = await call('group_call_state', {'conversation_id': conversationId});
    return r is Map ? GroupCallInfo.fromJson(Json.from(r)) : null;
  }

  /// WebRTC signaling to one participant ("offer" only to larger (account, device) pairs).
  Future<void> groupCallSignal({
    required String conversationId,
    required String callId,
    required String accountId,
    required int deviceId,
    required String kind,
    Object? data,
  }) =>
      call('group_call_signal', {
        'conversation_id': conversationId,
        'call_id': callId,
        'account_id': accountId,
        'device_id': deviceId,
        'kind': kind,
        'data': data,
      });

  /// Adds the call to the conversation history.
  Future<void> recordCall({
    required String conversationId,
    required String callId,
    required String callerAccountId,
    required String status,
    required int durationMs,
    required int startedAt,
  }) =>
      call('record_call', {
        'conversation_id': conversationId,
        'call_id': callId,
        'caller_account_id': callerAccountId,
        'status': status,
        'duration_ms': durationMs,
        'started_at': startedAt,
      });

  /// STUN / TURN servers for WebRTC (`{urls, username?, credential?}` maps).
  Future<List<Json>> getIceServers() async {
    final r = await _obj('get_ice_servers');
    return (r['ice_servers'] as List? ?? const []).whereType<Map>().map(Json.from).toList();
  }

  /// Unlinks this device on the server and wipes all local data.
  Future<void> logout() => call('logout');

  // --- accounts saved on this phone (local profiles, chat_core::profiles) ------
  // Opening or closing a profile swaps the core under the app: the session state is reloaded
  // afterwards (state/profiles.dart).
  Future<List<ProfileCard>> listProfiles() async => (await _list('profiles_list')).map(ProfileCard.fromJson).toList();
  Future<void> openProfile({required String id, required String password}) => call('profile_open', {'id': id, 'password': password});
  Future<void> openProfileByNumber({required String number, required String password}) =>
      call('profile_open_by_number', {'number': number, 'password': password});
  /// Quick switch to an account opened with its password since the app started
  /// (error "password_required" otherwise).
  Future<void> switchProfile(String id) => call('profile_switch', {'id': id});
  /// Locks the active account (it stays on this phone, logged in) and starts a blank profile.
  Future<void> closeProfile() => call('profile_close');
  /// Deletes the local data of a profile that is not active (forgotten password).
  Future<void> forgetProfile(String id) => call('profile_forget', {'id': id});
  /// Remote wipe: deletes the saved accounts this phone was removed from; returns their ids.
  Future<List<String>> checkRevoked() async =>
      ((await _obj('profiles_check_revoked'))['wiped'] as List? ?? const []).map((e) => '$e').toList();
  Future<CurrentProfile> currentProfile() async => CurrentProfile.fromJson(await _obj('profile_current'));
  Future<void> setProfileVisible(bool visible) => call('profile_set_visible', {'visible': visible});
  /// Masks the voice in a WAV recording (base64); returns a 16 kHz mono WAV (base64).
  /// Runs in the core (crates/chat-core/src/voice_mask.rs), works without an account.
  Future<String> maskVoice(String wavBase64) async => '${(await _obj('voice_mask', {'data_base64': wavBase64}))['data_base64']}';
  Future<String> getConnectionState() async => '${await call('get_connection_state')}';

  // --- device linking by QR code ---------------------------------------------
  // New device shows the code:   linkCreate -> show QR -> linkWait (resolves when approved).
  // New device scans a code:     linkClaim(link) -> linkWait.
  // Existing device scans:       linkInspect(link) -> confirm device name -> linkApprove(link).
  // Existing device shows code:  linkOffer -> show QR -> linkOfferWait (device name) -> confirm -> linkApprove().
  Future<String> linkCreate({required String serverUrl, required String deviceName}) async =>
      '${await call('link_create', {'server_url': serverUrl, 'device_name': deviceName})}';
  Future<void> linkClaim({required String link, required String deviceName}) =>
      call('link_claim', {'link': link, 'device_name': deviceName});
  Future<AccountInfo> linkWait() async => AccountInfo.fromJson(await _obj('link_wait'));
  Future<void> linkCancel() => call('link_cancel');
  Future<String> linkOffer() async => '${await call('link_offer')}';
  Future<String> linkOfferWait() async => '${await call('link_offer_wait')}';
  /// Verification code both screens show (after linkClaim / linkOfferWait).
  Future<String?> linkCode() async {
    final v = await call('link_code');
    return v == null ? null : '$v';
  }
  Future<String> linkInspect(String link) async => '${await call('link_inspect', {'link': link})}';
  Future<void> linkApprove({String? link}) => call('link_approve', {'link': ?link});

  /// Checks the account password locally (app lock).
  Future<bool> verifyPassword(String password) async => (await call('verify_password', {'password': password})) == true;

  /// The user confirmed the recovery phrase; the core forgets it.
  Future<void> confirmMnemonicSaved() => call('confirm_mnemonic_saved');

  // --- account ----------------------------------------------------------------
  Future<AccountDetails> getAccountDetails() async => AccountDetails.fromJson(await _obj('get_account_details'));
  Future<void> changePassword({required String oldPassword, required String newPassword}) =>
      call('change_password', {'old_password': oldPassword, 'new_password': newPassword});
  Future<void> deleteAccount(String password) => call('delete_account', {'password': password});
  Future<List<String>> revealMnemonic(String password) async =>
      ((await call('reveal_mnemonic', {'password': password})) as List).map((e) => '$e').toList();
  Future<PrivacySettings> setPrivacy({String? lastSeen, bool? readReceipts, bool? typingIndicators}) async =>
      PrivacySettings.fromJson(await _obj('set_privacy', {
        'last_seen': ?lastSeen,
        'read_receipts': ?readReceipts,
        'typing_indicators': ?typingIndicators,
      }));
  /// Do not disturb (synced to our other devices): its end in ms, 0 = off.
  Future<int> getDnd() async => _dndUntil(await _obj('get_dnd'));
  /// `until`: end (ms), 0 = off, [kDndForever] = until turned off. Returns the stored end.
  Future<int> setDnd(int until) async => _dndUntil(await _obj('set_dnd', {'until': until}));
  /// Call privacy settings (synced to our other devices).
  Future<CallSettings> getCallSettings() async => CallSettings.fromJson(await _obj('get_call_settings'));
  /// Absent fields stay as they are. Returns the stored settings.
  Future<CallSettings> setCallSettings({bool? allowCalls, bool? relayOnly}) async =>
      CallSettings.fromJson(await _obj('set_call_settings', {'allow_calls': ?allowCalls, 'relay_only': ?relayOnly}));
  /// Media retention period (synced to our other devices).
  Future<MediaRetention> getMediaRetention() async => MediaRetention.fromJson(await _obj('get_media_retention'));
  /// 1, 7, 14 or 30 days (anything else is refused); older media is erased at once.
  Future<MediaRetention> setMediaRetention(int days) async => MediaRetention.fromJson(await _obj('set_media_retention', {'days': days}));
  /// Timer of the chats we start and groups we create (seconds, 0 = off; synced to our other devices).
  Future<int> getDefaultDisappearing() async => _defaultTimer(await _obj('get_default_disappearing'));
  /// One of the offered timers (anything else is refused); existing chats keep their own.
  Future<int> setDefaultDisappearing(int seconds) async => _defaultTimer(await _obj('set_default_disappearing', {'seconds': seconds}));
  /// Removes stray and too old media files of the core now.
  Future<MediaCheck> runMediaCheck() async => MediaCheck.fromJson(await _obj('run_media_check'));
  Future<void> setBackupEnabled(bool enabled) => call('set_backup_enabled', {'enabled': enabled});
  Future<void> requestPhoneCode(String phone) => call('request_phone_code', {'phone': phone});
  Future<void> verifyPhone({required String phone, required String code}) => call('verify_phone', {'phone': phone, 'code': code});
  Future<void> removePhone() => call('remove_phone');
  Future<void> setDiscoverable(bool discoverable) => call('set_discoverable', {'discoverable': discoverable});
  Future<List<DeviceInfo>> listDevices() async => (await _list('list_devices')).map(DeviceInfo.fromJson).toList();
  Future<void> unlinkDevice(int deviceId) => call('unlink_device', {'device_id': deviceId});
  Future<void> renameDevice(String name) => call('rename_device', {'name': name});

  // --- account security (crates/chat-core/src/engine/security.rs) ----------------
  /// Devices added to the account since this one joined (event: device_alerts).
  Future<List<NewDeviceAlert>> deviceAlerts() async => (await _list('device_alerts')).map(NewDeviceAlert.fromJson).toList();
  Future<void> dismissDeviceAlerts() => call('dismiss_device_alerts');
  /// After `login` failed with "approval_required": resolves once another device approved
  /// (errors: "login_denied", "gone" = expired or cancelled).
  Future<AccountInfo> loginWaitApproval() async => AccountInfo.fromJson(await _obj('login_wait_approval'));
  Future<void> loginCancelApproval() => call('login_cancel_approval');
  Future<List<LoginRequestInfo>> listLoginRequests() async => (await _list('list_login_requests')).map(LoginRequestInfo.fromJson).toList();
  Future<void> answerLoginRequest(String id, bool approve) => call('answer_login_request', {'id': id, 'approve': approve});
  Future<void> setLoginApproval(bool enabled) => call('set_login_approval', {'enabled': enabled});
  /// Reports an account for spam to the server and (by default) blocks it.
  Future<Contact> reportSpam(String accountId, {bool block = true}) async =>
      Contact.fromJson(await _obj('report_spam', {'account_id': accountId, 'block': block}));

  // --- key transparency (crates/chat-core/src/engine/keytrans.rs) -------------------
  Future<KtOverview> ktStatus() async => KtOverview.fromJson(await _obj('kt_status'));
  /// Checks the contact's key against the log now when it was not verified yet.
  Future<KtContactStatus> ktContactStatus(String accountId) async =>
      KtContactStatus.fromJson(await _obj('kt_contact_status', {'account_id': accountId}));
  Future<KtOverview> ktCheckNow() async => KtOverview.fromJson(await _obj('kt_check_now'));
  Future<void> ktDismissAlerts() => call('kt_dismiss_alerts');

  /// Registers the push token of this device: [provider] "fcm" (FCM registration token) or
  /// "apns_voip" (iOS PushKit token, calls only; an empty token removes it).
  Future<void> setPushToken({required String provider, required String token}) =>
      call('set_push_token', {'provider': provider, 'token': token});

  /// This account's push tag on this phone (pushes carry it as `p`).
  Future<String> pushTag() async => (await call('push_tag')) as String;

  /// A push arrived: connect now (or check the open socket) and fetch what waits.
  Future<void> pushWake() => call('push_wake');

  /// The app went to the background without a call in progress (true) or came back (false):
  /// in the background the core closes its connection and relies on pushes (engine/net.rs).
  Future<void> setBackground(bool background) => call('set_background', {'background': background});

  /// The user is at this device (app in the foreground) or not: the account shows online only
  /// while one of its devices is active (engine/net.rs).
  Future<void> setPresenceActive(bool active) => call('set_presence_active', {'active': active});

  // --- profile ----------------------------------------------------------------
  Future<Profile> getProfile() async => Profile.fromJson(await _obj('get_profile'));

  /// [avatarPath] "" removes the avatar; [avatarDataBase64] sets it from image bytes (camera).
  Future<Profile> setProfile({String? name, String? about, String? avatarPath, String? avatarDataBase64}) async =>
      Profile.fromJson(await _obj('set_profile', {
        'name': ?name,
        'about': ?about,
        'avatar_path': ?avatarPath,
        'avatar_data_base64': ?avatarDataBase64,
      }));

  /// Sets (or removes, with null) the public @username. Errors with "conflict" when taken.
  Future<String?> setUsername(String? username) async {
    final v = await call('set_username', {'username': username});
    return v is String ? v : null;
  }

  // --- contacts ---------------------------------------------------------------
  Future<List<Contact>> listContacts() async => (await _list('list_contacts')).map(Contact.fromJson).toList();
  Future<Contact> getContact(String accountId) async => Contact.fromJson(await _obj('get_contact', {'account_id': accountId}));
  Future<Contact> addContactByNumber(String number, {String? name}) async =>
      Contact.fromJson(await _obj('add_contact_by_number', {'number': number, 'name': ?name}));
  /// Finds an account by 16-digit number or @username (null when there is none).
  Future<FoundAccount?> findAccount(String query) async {
    final v = await call('find_account', {'query': query});
    return v is Map ? FoundAccount.fromJson(Json.from(v)) : null;
  }

  /// Adds a contact by account number or @username.
  Future<Contact> addContactByQuery(String query, {String? name}) async =>
      Contact.fromJson(await _obj('add_contact_by_query', {'query': query, 'name': ?name}));

  /// Blocked contacts.
  Future<List<Contact>> listBlocked() async => (await _list('list_blocked')).map(Contact.fromJson).toList();

  Future<Contact> renameContact(String accountId, String? name) async =>
      Contact.fromJson(await _obj('rename_contact', {'account_id': accountId, 'name': name}));
  Future<Contact> setBlocked(String accountId, bool blocked) async =>
      Contact.fromJson(await _obj('set_blocked', {'account_id': accountId, 'blocked': blocked}));

  /// Imports a .vcf or .csv address book file and matches phone numbers on the server.
  Future<ContactImportResult> importContacts(String path) async =>
      ContactImportResult.fromJson(await _obj('import_contacts', {'path': path}));

  /// Payload of the "add me" QR code: `chatapp://add?n=<number>&s=<server_url>`.
  Future<String> getMyContactLink() async => '${await call('get_my_contact_link')}';
  /// Adds a contact from a scanned/pasted link: number link or username link.
  Future<Contact> addContactFromLink(String link) async => Contact.fromJson(await _obj('add_contact_from_link', {'link': link}));

  /// Username link / QR (no account number inside); created on first use, null without a username.
  Future<UsernameLink?> getUsernameLink() async {
    final v = await call('get_username_link');
    return v is Map ? UsernameLink.fromJson(Json.from(v)) : null;
  }

  /// New username link; links shared before stop working.
  Future<UsernameLink?> resetUsernameLink() async {
    final v = await call('reset_username_link');
    return v is Map ? UsernameLink.fromJson(Json.from(v)) : null;
  }

  /// The account a username link leads to (without adding it).
  Future<FoundAccount> resolveUsernameLink(String link) async =>
      FoundAccount.fromJson(await _obj('resolve_username_link', {'link': link}));
  Future<SafetyNumber> getSafetyNumber(String accountId) async =>
      SafetyNumber.fromJson(await _obj('get_safety_number', {'account_id': accountId}));
  Future<Contact> setVerified(String accountId, bool verified) async =>
      Contact.fromJson(await _obj('set_verified', {'account_id': accountId, 'verified': verified}));
  Future<void> watchPresence(List<String> accountIds) => call('watch_presence', {'account_ids': accountIds});

  // --- conversations ----------------------------------------------------------
  Future<List<Conversation>> listConversations() async => (await _list('list_conversations')).map(Conversation.fromJson).toList();
  Future<Conversation> getConversation(String id) async => Conversation.fromJson(await _obj('get_conversation', {'conversation_id': id}));

  /// Opens (or creates) the direct conversation with an account.
  Future<Conversation> openDirectConversation(String accountId) async =>
      Conversation.fromJson(await _obj('open_direct_conversation', {'account_id': accountId}));

  /// Accepts a message request, or joins a group we were added to by a stranger.
  Future<Conversation> acceptConversation(String conversationId) async =>
      Conversation.fromJson(await _obj('accept_conversation', {'conversation_id': conversationId}));
  Future<Conversation> setConversationSettings(String conversationId, ConversationSettingsPatch patch) async =>
      Conversation.fromJson(await _obj('set_conversation_settings', {'conversation_id': conversationId, ...patch.toJson()}));
  Future<void> setDraft(String conversationId, String? draft) => call('set_draft', {'conversation_id': conversationId, 'draft': draft});

  /// Deletes the local conversation and its messages (does not leave groups).
  Future<void> deleteConversation(String conversationId) => call('delete_conversation', {'conversation_id': conversationId});
  Future<void> clearConversation(String conversationId) => call('clear_conversation', {'conversation_id': conversationId});

  /// Group admins only: clears the history for every member (the group stays).
  /// Destructive "for everyone" actions and wiping require the account password (checked by the core).
  Future<void> clearGroupForEveryone(String conversationId, String password) =>
      call('clear_group_for_everyone', {'conversation_id': conversationId, 'password': password});
  /// Direct chat: deleted on both sides (the peer applies it when online). Group: our messages for everyone.
  Future<void> deleteConversationForEveryone(String conversationId, String password) =>
      call('delete_conversation_for_everyone', {'conversation_id': conversationId, 'password': password});
  /// Every conversation, here and for the other side of each one.
  /// Opens the password-locked local database (app start).
  Future<void> unlock(String password) => call('unlock', {'password': password});
  /// Locks the local data with the (checked) account password, or re-locks it with a new one.
  Future<void> protectLocalData(String password) => call('protect_local_data', {'password': password});
  /// Forgotten password: deletes all local data (emits logged_out with "local_reset").
  /// Without a password only while the data is locked; once unlocked the core requires it.
  Future<void> resetLocalData([String? password]) => call('reset_local_data', password == null ? null : {'password': password});
  /// Decrypted bytes of one of our media files (they are encrypted at rest).
  Future<Uint8List> readMedia(String path) async => base64Decode(await call('read_media', {'path': path}) as String);
  /// Writes a decrypted copy of one of our media files to [dest].
  Future<void> exportMedia(String path, String dest) => call('export_media', {'path': path, 'dest': dest});
  Future<void> wipeAllConversations(String password) => call('wipe_all_conversations', {'password': password});
  /// Temporary mode: conversations live in memory only and are gone after a restart.
  Future<bool> getTemporaryMode() async => await call('get_temporary_mode', null) == true;
  Future<void> setTemporaryMode(bool enabled) => call('set_temporary_mode', {'enabled': enabled});

  // --- messages ---------------------------------------------------------------
  /// Newest first when [before] is omitted; returns up to [limit] messages older than [before].
  Future<List<Message>> getMessages(String conversationId, {int? before, int? limit}) async =>
      (await _list('get_messages', {'conversation_id': conversationId, 'before': ?before, 'limit': ?limit}))
          .map(Message.fromJson)
          .toList();
  Future<Message> sendMessage({
    required String conversationId,
    String? text,
    String? replyTo,
    List<OutgoingAttachment>? attachments,
    Location? location,
    List<String>? contactIds,
    List<Mention>? mentions,
    bool viewOnce = false,
  }) async =>
      Message.fromJson(await _obj('send_message', {
        'conversation_id': conversationId,
        'text': ?text,
        'reply_to': ?replyTo,
        if (attachments != null && attachments.isNotEmpty) 'attachments': attachments.map((a) => a.toJson()).toList(),
        'location': ?location?.toJson(),
        if (contactIds != null && contactIds.isNotEmpty) 'contact_ids': contactIds,
        if (mentions != null && mentions.isNotEmpty) 'mentions': mentions.map((m) => m.toJson()).toList(),
        if (viewOnce) 'view_once': true,
      }));
  /// Opens incoming view-once media (once): marks it opened everywhere and returns the
  /// attachment with its local path. Call [viewOnceDone] when the viewer closes.
  Future<Attachment> openViewOnce(String messageId) async =>
      Attachment.fromJson(await _obj('open_view_once', {'message_id': messageId}));
  /// The view-once viewer closed: the core securely deletes the media.
  Future<void> viewOnceDone(String messageId) => call('view_once_done', {'message_id': messageId});
  Future<Message> editMessage(String messageId, String text, {List<Mention> mentions = const []}) async => Message.fromJson(
      await _obj('edit_message', {'message_id': messageId, 'text': text, 'mentions': mentions.map((m) => m.toJson()).toList()}));
  /// Every version of an edited message, oldest first (empty if it was never edited).
  Future<List<EditVersion>> getEditHistory(String messageId) async =>
      (await _list('get_edit_history', {'message_id': messageId})).map(EditVersion.fromJson).toList();
  // Pinned and starred messages (this device only).
  /// [seconds]: 86400, 604800 or 2592000; null unpins.
  Future<Message> pinMessage(String messageId, int? seconds) async =>
      Message.fromJson(await _obj('pin_message', {'message_id': messageId, 'seconds': seconds}));
  Future<Message> starMessage(String messageId, bool starred) async =>
      Message.fromJson(await _obj('star_message', {'message_id': messageId, 'starred': starred}));
  Future<List<Message>> pinnedMessages(String conversationId) async =>
      (await _list('pinned_messages', {'conversation_id': conversationId})).map(Message.fromJson).toList();
  Future<List<Message>> starredMessages() async => (await _list('starred_messages')).map(Message.fromJson).toList();
  Future<void> deleteMessage(String messageId, {required bool forEveryone}) =>
      call('delete_message', {'message_id': messageId, 'for_everyone': forEveryone});
  Future<Message> react(String messageId, String? emoji) async =>
      Message.fromJson(await _obj('react', {'message_id': messageId, 'emoji': emoji}));
  Future<void> forwardMessages(List<String> messageIds, List<String> conversationIds) =>
      call('forward_messages', {'message_ids': messageIds, 'conversation_ids': conversationIds});
  Future<void> markRead(String conversationId) => call('mark_read', {'conversation_id': conversationId});
  Future<void> sendTyping(String conversationId, bool typing) =>
      call('send_typing', {'conversation_id': conversationId, 'typing': typing});
  Future<Message> retryMessage(String messageId) async => Message.fromJson(await _obj('retry_message', {'message_id': messageId}));
  Future<MessageInfo> getMessageInfo(String messageId) async =>
      MessageInfo.fromJson(await _obj('get_message_info', {'message_id': messageId}));
  Future<void> downloadAttachment(String messageId, String attachmentId) =>
      call('download_attachment', {'message_id': messageId, 'attachment_id': attachmentId});

  /// Copies a decrypted attachment to a user chosen path.
  Future<void> saveAttachment(String messageId, String attachmentId, String destPath) =>
      call('save_attachment', {'message_id': messageId, 'attachment_id': attachmentId, 'dest_path': destPath});
  Future<List<Message>> searchMessages(String query, {String? conversationId, int? limit}) async =>
      (await _list('search_messages', {'query': query, 'conversation_id': ?conversationId, 'limit': ?limit}))
          .map(Message.fromJson)
          .toList();

  /// Media gallery of a conversation. [kind] is "media" or "files".
  Future<List<Message>> listMedia(String conversationId, String kind, {int? limit}) async =>
      (await _list('list_media', {'conversation_id': conversationId, 'kind': kind, 'limit': ?limit}))
          .map(Message.fromJson)
          .toList();

  // --- groups -----------------------------------------------------------------
  Future<Conversation> createGroup({
    required String name,
    String? description,
    required List<String> memberAccountIds,
    String? avatarPath,
    GroupSettings? settings,
  }) async =>
      Conversation.fromJson(await _obj('create_group', {
        'name': name,
        'description': ?description,
        'member_account_ids': memberAccountIds,
        'avatar_path': ?avatarPath,
        'settings': ?settings?.toJson(),
      }));
  // Group invite links.
  Future<GroupInfo> setGroupInvite(String conversationId, {required bool enabled, bool? approval, bool reset = false}) async =>
      GroupInfo.fromJson(await _obj('set_group_invite', {
        'conversation_id': conversationId,
        'enabled': enabled,
        'approval': ?approval,
        'reset': reset,
      }));
  Future<InvitePreview> previewGroupInvite(String link) async => InvitePreview.fromJson(await _obj('preview_group_invite', {'link': link}));
  Future<JoinResult> joinGroupByLink(String link) async => JoinResult.fromJson(await _obj('join_group_by_link', {'link': link}));
  Future<void> cancelJoinRequest(String groupId) => call('cancel_join_request', {'group_id': groupId});
  Future<List<JoinRequest>> groupJoinRequests(String conversationId) async =>
      (await _list('group_join_requests', {'conversation_id': conversationId})).map(JoinRequest.fromJson).toList();
  Future<GroupInfo> approveJoinRequest(String conversationId, String accountId) async =>
      GroupInfo.fromJson(await _obj('approve_join_request', {'conversation_id': conversationId, 'account_id': accountId}));
  Future<GroupInfo> rejectJoinRequest(String conversationId, String accountId) async =>
      GroupInfo.fromJson(await _obj('reject_join_request', {'conversation_id': conversationId, 'account_id': accountId}));

  /// Admins: accounts that can't rejoin by link (removed by an admin, or request rejected).
  Future<List<GroupBan>> groupBans(String conversationId) async =>
      (await _list('group_bans', {'conversation_id': conversationId})).map(GroupBan.fromJson).toList();
  Future<GroupInfo> unbanGroupMember(String conversationId, String accountId) async =>
      GroupInfo.fromJson(await _obj('unban_group_member', {'conversation_id': conversationId, 'account_id': accountId}));

  Future<GroupInfo> getGroupInfo(String conversationId) async =>
      GroupInfo.fromJson(await _obj('get_group_info', {'conversation_id': conversationId}));

  /// [avatarPath] "" removes the avatar.
  Future<GroupInfo> updateGroup(
    String conversationId, {
    String? name,
    String? description,
    String? avatarPath,
    GroupSettings? settings,
  }) async =>
      GroupInfo.fromJson(await _obj('update_group', {
        'conversation_id': conversationId,
        'name': ?name,
        'description': ?description,
        'avatar_path': ?avatarPath,
        'settings': ?settings?.toJson(),
      }));
  Future<GroupInfo> addGroupMembers(String conversationId, List<String> accountIds) async =>
      GroupInfo.fromJson(await _obj('add_group_members', {'conversation_id': conversationId, 'account_ids': accountIds}));
  Future<GroupInfo> removeGroupMember(String conversationId, String accountId) async =>
      GroupInfo.fromJson(await _obj('remove_group_member', {'conversation_id': conversationId, 'account_id': accountId}));
  Future<GroupInfo> setGroupAdmin(String conversationId, String accountId, bool admin) async =>
      GroupInfo.fromJson(await _obj('set_group_admin', {'conversation_id': conversationId, 'account_id': accountId, 'admin': admin}));
  Future<void> leaveGroup(String conversationId) => call('leave_group', {'conversation_id': conversationId});
}
