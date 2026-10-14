// Android: a call in progress runs a foreground service (native side:
// android/app/src/main/kotlin/pm/dialog/chat/CallService.kt, channel "dialog/call_service").
// Without it Android cuts the microphone and camera of an app in the background (screen locked)
// and may freeze or kill the process: the call goes silent, then drops. The service shows an
// ongoing "Call in progress" notification with "Hang up"; tapping it opens the app.
//
// Also the proximity wake lock of voice calls (screen off at the ear, app not paused).
//
// Every method is a no-op off Android.

import 'dart:async';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

class OngoingCall {
  OngoingCall._();

  static const _channel = MethodChannel('dialog/call_service');

  /// The user tapped "Hang up" on the notification.
  static void Function()? onHangUp;

  static bool get supported => !kIsWeb && Platform.isAndroid;

  static bool _listening = false;

  static Future<void> _invoke(String method, [Object? args]) async {
    if (!supported) return;
    if (!_listening) {
      _listening = true;
      _channel.setMethodCallHandler((call) async {
        if (call.method == 'hangUp') onHangUp?.call();
        return null;
      });
    }
    try {
      await _channel.invokeMethod<void>(method, args);
    } catch (e) {
      debugPrint('call service $method: $e');
    }
  }

  /// Starts (or updates) the foreground service of the call in progress.
  static Future<void> start({
    required String title,
    required String text,
    required String hangUp,
    required String channelName,
    required bool video,
  }) =>
      _invoke('start', {'title': title, 'text': text, 'hangUp': hangUp, 'channelName': channelName, 'video': video});

  static Future<void> stop() => _invoke('stop');

  /// Screen off when the phone is at the ear (voice call on the earpiece).
  static Future<void> setProximity(bool on) => _invoke('setProximity', on);
}
