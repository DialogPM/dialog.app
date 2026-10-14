// Optional push notifications through Firebase Cloud Messaging, plus iOS VoIP pushes for calls
// (services/callkit.dart). Setup: docs/PUSH-SETUP.md.
//
// Firebase is only used when the app was built with a Firebase configuration
// (android/app/google-services.json, ios/Runner/GoogleService-Info.plist). Without it
// Firebase.initializeApp() fails and push is simply disabled: the app still receives messages
// and shows notifications while it runs.
//
// Pushes carry no content by design (no sender, no conversation, no text): only "new_message"
// or "call" (with the call id) and `p`, the push tag of the account on this phone it is for.
// - iOS shows the alert itself, with fixed localized text (loc-keys NEW_MESSAGE /
//   INCOMING_CALL in ios/Runner/*.lproj). A Notification Service Extension could fetch and
//   decrypt first; not done yet (docs/PUSH-SETUP.md).
// - Android gets data-only messages. While the app process runs, the background isolate hands
//   them to the main isolate (port [kPushPortName]) which fetches, decrypts and notifies as
//   usual (honoring previews, muted chats and do-not-disturb; state/push_handling.dart). When
//   the app is not running no account is open, and its data cannot be read without the
//   password: a generic "New message" / "Incoming call" is shown.

import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:isolate';
import 'dart:ui';

import 'package:firebase_core/firebase_core.dart';
import 'package:firebase_messaging/firebase_messaging.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_local_notifications/flutter_local_notifications.dart';
import 'package:path_provider/path_provider.dart';

import '../l10n/i18n.dart';
import 'callkit.dart';
import 'notifications.dart';

/// Name of the main isolate's port for pushes and notification actions handled elsewhere.
const kPushPortName = 'dialog/push';
/// Decisions taken while the app was not running (call id -> "decline"), applied once the
/// account is open (state/push_handling.dart).
const kCallActionsFile = 'call_actions.json';

/// App-wide preferences the background code needs (mirrored in prefs.json by state/prefs.dart).
class PushPrefs {
  const PushPrefs({required this.language, required this.notifications, required this.dndUntil});

  final String language;
  final bool notifications;
  final int dndUntil;

  bool get dnd => dndUntil > DateTime.now().millisecondsSinceEpoch;

  static Future<PushPrefs> load() async {
    var lang = systemLanguage();
    var notifications = true;
    var dnd = 0;
    try {
      final dir = await getApplicationSupportDirectory();
      final f = File('${dir.path}/prefs.json');
      if (f.existsSync()) {
        final j = jsonDecode(f.readAsStringSync());
        if (j is Map) {
          if (j['language'] is String) lang = j['language'] as String;
          if (j['notifications'] == false) notifications = false;
          if (j['dndUntil'] is int) dnd = j['dndUntil'] as int;
        }
      }
    } catch (_) {}
    return PushPrefs(language: lang, notifications: notifications, dndUntil: dnd);
  }
}

/// Shows what can be shown without an open account: "New message" / "Incoming call", or the
/// "in another account" variants (tapping those opens the accounts on this phone).
Future<void> showGenericPush(Map<String, dynamic> data, PushPrefs p, {bool otherAccount = false}) async {
  // Do not disturb: nothing rings and nothing is shown (like the in-app behavior).
  if (p.dnd) return;
  final lang = p.language;
  await Notifications.instance.init(
    channelName: translate(lang, 'settings.notifications.title'),
    callChannelName: translate(lang, 'call.incomingTitle'),
  );
  if (data['type'] == 'call') {
    final callId = data['call_id'] as String? ?? '';
    if (otherAccount) {
      await Notifications.instance.showSimple(kBrandName, translate(lang, 'mobile.pushOtherAccountCall'), payload: kProfilesPayload);
      return;
    }
    if (callId.isEmpty) return;
    await Notifications.instance.showIncomingCall(
      callId: callId,
      title: kBrandName,
      body: translate(lang, 'call.incomingTitle'),
      answerLabel: translate(lang, 'call.accept'),
      declineLabel: translate(lang, 'call.decline'),
    );
    return;
  }
  if (!p.notifications) return;
  await Notifications.instance.showSimple(
    kBrandName,
    translate(lang, otherAccount ? 'mobile.pushOtherAccount' : 'notifications.hidden'),
    payload: otherAccount ? kProfilesPayload : null,
  );
}

/// Handles data-only push messages while the app is in the background or terminated
/// (Android; a separate isolate).
@pragma('vm:entry-point')
Future<void> firebaseBackgroundHandler(RemoteMessage message) async {
  // Messages with a `notification` block are displayed by the system itself.
  if (message.notification != null) return;
  final data = Map<String, dynamic>.from(message.data);
  // The app runs: its main isolate knows which account is open and can fetch right away.
  final port = IsolateNameServer.lookupPortByName(kPushPortName);
  if (port != null) {
    port.send(data);
    return;
  }
  try {
    await Firebase.initializeApp();
  } catch (_) {}
  await showGenericPush(data, await PushPrefs.load());
}

/// Notification actions handled without opening the app (Decline on an incoming call).
@pragma('vm:entry-point')
Future<void> notificationActionBackground(NotificationResponse r) async {
  final p = r.payload ?? '';
  if (!p.startsWith(kCallPayload)) return;
  final callId = p.substring(kCallPayload.length);
  final action = r.actionId ?? '';
  final port = IsolateNameServer.lookupPortByName(kPushPortName);
  if (port != null) {
    port.send({'type': 'call_action', 'action': action, 'call_id': callId});
    return;
  }
  if (action != 'decline') return;
  // No account is open: the offer is declined once it is (state/push_handling.dart).
  try {
    WidgetsFlutterBinding.ensureInitialized();
    DartPluginRegistrant.ensureInitialized();
    final dir = await getApplicationSupportDirectory();
    final f = File('${dir.path}/$kCallActionsFile');
    final m = f.existsSync() ? Map<String, dynamic>.from(jsonDecode(f.readAsStringSync()) as Map) : <String, dynamic>{};
    m[callId] = {'action': 'decline', 'at': DateTime.now().millisecondsSinceEpoch};
    f.writeAsStringSync(jsonEncode(m));
  } catch (_) {}
}

class Push {
  Push._();

  static final Push instance = Push._();

  bool _available = false;
  StreamSubscription<String>? _refreshSub;
  final _subs = <StreamSubscription<dynamic>>[];
  ReceivePort? _port;
  Map<String, dynamic>? _launchData;

  /// A push (or a notification action from the background) reached the running app:
  /// [background] is true when it came through the background isolate.
  void Function(Map<String, dynamic> data, {required bool background})? onData;

  /// The user tapped a push notification shown by the system (iOS alerts).
  void Function(Map<String, dynamic> data)? onOpened;

  /// True when Firebase is configured and initialized.
  bool get available => _available;

  /// Initializes Firebase if the platform supports it and a configuration is bundled, the
  /// port for the background isolate, and the iOS VoIP bridge. Never throws.
  Future<void> init() async {
    if (kIsWeb || !(Platform.isAndroid || Platform.isIOS)) return;
    try {
      final port = ReceivePort();
      IsolateNameServer.removePortNameMapping(kPushPortName);
      IsolateNameServer.registerPortWithName(port.sendPort, kPushPortName);
      port.listen((m) {
        if (m is Map) onData?.call(Map<String, dynamic>.from(m), background: true);
      });
      _port = port;
    } catch (e) {
      debugPrint('push port: $e');
    }
    try {
      await CallKit.init();
    } catch (e) {
      debugPrint('voip bridge: $e');
    }
    try {
      await Firebase.initializeApp();
      FirebaseMessaging.onBackgroundMessage(firebaseBackgroundHandler);
      _subs.add(FirebaseMessaging.onMessage.listen((m) => onData?.call(Map<String, dynamic>.from(m.data), background: false)));
      _subs.add(FirebaseMessaging.onMessageOpenedApp.listen((m) => onOpened?.call(Map<String, dynamic>.from(m.data))));
      final initial = await FirebaseMessaging.instance.getInitialMessage();
      if (initial != null) _launchData = Map<String, dynamic>.from(initial.data);
      _available = true;
    } catch (e) {
      debugPrint('push disabled (Firebase not configured): $e');
      _available = false;
    }
  }

  /// Data of the push notification that launched the app (taken once).
  Map<String, dynamic>? takeLaunchData() {
    final d = _launchData;
    _launchData = null;
    return d;
  }

  /// Registers this device's push tokens with the server (after an account is opened) and
  /// keeps them updated: the FCM token, and on iOS the PushKit token for calls.
  Future<void> register(Future<void> Function(String provider, String token) send) async {
    if (CallKit.supported) {
      CallKit.onToken = (t) => unawaited(send('apns_voip', t).catchError((_) {}));
      final voip = await CallKit.voipToken();
      if (voip != null && voip.isNotEmpty) unawaited(send('apns_voip', voip).catchError((_) {}));
    }
    if (!_available) return;
    try {
      final m = FirebaseMessaging.instance;
      await m.requestPermission(alert: true, badge: true, sound: true);
      if (Platform.isIOS) await m.getAPNSToken();
      final token = await m.getToken();
      if (token != null) await send('fcm', token);
      await _refreshSub?.cancel();
      _refreshSub = m.onTokenRefresh.listen((t) => unawaited(send('fcm', t).catchError((_) {})));
    } catch (e) {
      debugPrint('push registration failed: $e');
    }
  }

  @visibleForTesting
  void dispose() {
    for (final s in _subs) {
      unawaited(s.cancel());
    }
    _subs.clear();
    _port?.close();
    IsolateNameServer.removePortNameMapping(kPushPortName);
  }
}
