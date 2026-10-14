// Android battery optimization: calls and incoming messages must not be cut by the system's
// battery saver (Doze, OEM "app killers"). Messaging/VoIP apps may ask for the exemption
// (Play policy); asked once after sign-in, and available any time in Settings → Notifications.
// No-op on other platforms.

import 'dart:io';

import 'package:flutter/services.dart';

class BatteryOptimization {
  BatteryOptimization._();

  static const _channel = MethodChannel('dialog/battery');

  /// True when the app is already exempt (or the platform has no such optimization).
  static Future<bool> isExempt() async {
    if (!Platform.isAndroid) return true;
    try {
      return await _channel.invokeMethod<bool>('isIgnoring') ?? true;
    } catch (_) {
      return true;
    }
  }

  /// Shows the system dialog asking for the exemption (falls back to the app's battery settings).
  static Future<void> request() async {
    if (!Platform.isAndroid) return;
    try {
      await _channel.invokeMethod<void>('request');
    } catch (_) {}
  }
}
