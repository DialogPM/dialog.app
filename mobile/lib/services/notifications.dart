// Local notifications for new incoming messages (while the app runs) and for push messages
// received in the background. Tapping a notification opens its chat. Incoming calls on Android
// use their own channel and a full-screen notification with Answer / Decline.

import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter_local_notifications/flutter_local_notifications.dart';

import 'push.dart' show notificationActionBackground;

const _channelId = 'messages';
const _callChannelId = 'calls';
/// One call rings at a time.
const _callNotificationId = 2;
/// Payload of a notification that opens the list of accounts on this phone.
const kProfilesPayload = 'profiles';
/// Payload prefix of an incoming-call notification (followed by the call id).
const kCallPayload = 'call:';

class Notifications {
  Notifications._();

  static final Notifications instance = Notifications._();

  final _plugin = FlutterLocalNotificationsPlugin();
  bool _ready = false;

  /// Called with the conversation id when a notification is tapped.
  void Function(String conversationId)? onOpen;

  /// Conversation of the notification that launched the app, if any.
  String? launchConversation;

  /// Called with the action ("answer", "decline", or "" for a tap) of an incoming-call
  /// notification.
  void Function(String action, String callId)? onCallAction;

  /// Called when a notification for another account on this phone is tapped.
  void Function()? onOpenProfiles;

  /// Incoming-call action that launched the app (action, call id), if any.
  (String, String)? launchCallAction;

  bool get supported => !kIsWeb && (Platform.isAndroid || Platform.isIOS || Platform.isLinux || Platform.isMacOS);

  void _onResponse(NotificationResponse r) {
    final p = r.payload ?? '';
    if (p.startsWith(kCallPayload)) {
      onCallAction?.call(r.actionId ?? '', p.substring(kCallPayload.length));
    } else if (p == kProfilesPayload) {
      onOpenProfiles?.call();
    } else if (p.isNotEmpty) {
      onOpen?.call(p);
    }
  }

  Future<void> init({String channelName = 'Messages', String callChannelName = 'Calls'}) async {
    if (_ready || !supported) return;
    // Linux needs a D-Bus session (headless test machines have none).
    if (Platform.isLinux && (Platform.environment['DBUS_SESSION_BUS_ADDRESS'] ?? '').isEmpty) return;
    try {
      await _plugin.initialize(
        settings: InitializationSettings(
          android: const AndroidInitializationSettings('@mipmap/ic_launcher'),
          iOS: const DarwinInitializationSettings(
            requestAlertPermission: false,
            requestBadgePermission: false,
            requestSoundPermission: false,
          ),
          macOS: const DarwinInitializationSettings(),
          linux: const LinuxInitializationSettings(defaultActionName: 'Open'),
        ),
        onDidReceiveNotificationResponse: _onResponse,
        // Decline on an incoming-call notification runs without opening the app.
        onDidReceiveBackgroundNotificationResponse: notificationActionBackground,
      );
      final android = _plugin.resolvePlatformSpecificImplementation<AndroidFlutterLocalNotificationsPlugin>();
      await android?.createNotificationChannel(AndroidNotificationChannel(
        _channelId,
        channelName,
        importance: Importance.high,
      ));
      await android?.createNotificationChannel(AndroidNotificationChannel(
        _callChannelId,
        callChannelName,
        importance: Importance.max,
        sound: const UriAndroidNotificationSound('content://settings/system/ringtone'),
        audioAttributesUsage: AudioAttributesUsage.notificationRingtone,
      ));
      final launch = Platform.isAndroid || Platform.isIOS ? await _plugin.getNotificationAppLaunchDetails() : null;
      if (launch?.didNotificationLaunchApp == true) {
        final r = launch!.notificationResponse;
        final p = r?.payload;
        if (p != null && p.startsWith(kCallPayload)) {
          launchCallAction = (r?.actionId ?? '', p.substring(kCallPayload.length));
        } else if (p != null && p.isNotEmpty && p != kProfilesPayload) {
          // (A notification of another account: the app starts on the accounts list anyway.)
          launchConversation = p;
        }
      }
      _ready = true;
    } catch (e) {
      debugPrint('notifications unavailable: $e');
    }
  }

  /// Asks for the permission to show notifications (Android 13+, iOS).
  Future<bool> requestPermission() async {
    if (!_ready) return false;
    try {
      if (Platform.isAndroid) {
        final android = _plugin.resolvePlatformSpecificImplementation<AndroidFlutterLocalNotificationsPlugin>();
        return await android?.requestNotificationsPermission() ?? true;
      }
      if (Platform.isIOS) {
        final ios = _plugin.resolvePlatformSpecificImplementation<IOSFlutterLocalNotificationsPlugin>();
        return await ios?.requestPermissions(alert: true, badge: true, sound: true) ?? false;
      }
    } catch (_) {}
    return true;
  }

  /// Whether the system allows notifications (null when unknown).
  Future<bool?> enabled() async {
    if (!_ready) return null;
    try {
      if (Platform.isAndroid) {
        return await _plugin.resolvePlatformSpecificImplementation<AndroidFlutterLocalNotificationsPlugin>()?.areNotificationsEnabled();
      }
    } catch (_) {}
    return null;
  }

  int _idFor(String conversationId) => conversationId.hashCode & 0x7fffffff;

  Future<void> showMessage({
    required String conversationId,
    required String title,
    required String body,
    bool sound = true,
  }) async {
    if (!_ready) return;
    try {
      await _plugin.show(
        id: _idFor(conversationId),
        title: title,
        body: body,
        payload: conversationId,
        notificationDetails: NotificationDetails(
          android: AndroidNotificationDetails(
            _channelId,
            'Messages',
            importance: Importance.high,
            priority: Priority.high,
            playSound: sound,
            category: AndroidNotificationCategory.message,
            // Lock screen: "Dialog" without title or text (the content is shown only once the
            // device is unlocked). With the preview off the content is generic anyway.
            visibility: NotificationVisibility.private,
          ),
          iOS: DarwinNotificationDetails(presentSound: sound, presentAlert: true, presentBanner: true),
          linux: LinuxNotificationDetails(suppressSound: !sound),
        ),
      );
    } catch (e) {
      debugPrint('notification failed: $e');
    }
  }

  /// Generic notification without content (push in the background, test notification).
  /// [payload]: [kProfilesPayload] opens the accounts on this phone when tapped.
  Future<void> showSimple(String title, String body, {String? payload}) async {
    if (!_ready) return;
    try {
      await _plugin.show(
        id: 1,
        title: title,
        body: body,
        payload: payload,
        notificationDetails: const NotificationDetails(
          android: AndroidNotificationDetails(_channelId, 'Messages',
              importance: Importance.high, priority: Priority.high, visibility: NotificationVisibility.private),
          iOS: DarwinNotificationDetails(),
        ),
      );
    } catch (_) {}
  }

  /// Incoming call (Android): rings on the calls channel, full screen over the lock screen,
  /// with Answer (opens the app) and Decline (handled in the background). [title] / [body]
  /// carry no caller identity unless notification previews are on.
  Future<void> showIncomingCall({
    required String callId,
    required String title,
    required String body,
    required String answerLabel,
    required String declineLabel,
  }) async {
    if (!_ready) return;
    try {
      await _plugin.show(
        id: _callNotificationId,
        title: title,
        body: body,
        payload: '$kCallPayload$callId',
        notificationDetails: NotificationDetails(
          android: AndroidNotificationDetails(
            _callChannelId,
            'Calls',
            importance: Importance.max,
            priority: Priority.max,
            category: AndroidNotificationCategory.call,
            fullScreenIntent: true,
            ongoing: true,
            autoCancel: false,
            // The caller gives up after 45 s.
            timeoutAfter: 45000,
            audioAttributesUsage: AudioAttributesUsage.notificationRingtone,
            // FLAG_INSISTENT: the ringtone repeats until answered or cancelled.
            additionalFlags: Int32List.fromList(const [4]),
            visibility: NotificationVisibility.private,
            actions: [
              AndroidNotificationAction('decline', declineLabel, cancelNotification: true),
              AndroidNotificationAction('answer', answerLabel, showsUserInterface: true, cancelNotification: true),
            ],
          ),
          iOS: const DarwinNotificationDetails(presentSound: true, presentAlert: true, presentBanner: true),
        ),
      );
    } catch (e) {
      debugPrint('call notification failed: $e');
    }
  }

  Future<void> cancelCall() async {
    if (!_ready) return;
    try {
      await _plugin.cancel(id: _callNotificationId);
    } catch (_) {}
  }

  Future<void> cancelFor(String conversationId) async {
    if (!_ready) return;
    try {
      await _plugin.cancel(id: _idFor(conversationId));
    } catch (_) {}
  }

  Future<void> cancelAll() async {
    if (!_ready) return;
    try {
      await _plugin.cancelAll();
    } catch (_) {}
  }
}
