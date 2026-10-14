// Platform hardening that needs native code (MainActivity.kt / AppDelegate.swift, channel
// "dialog/security"): screen security (Android FLAG_SECURE; iOS cover in the app switcher and
// while the screen is recorded), sensitive clipboard copies and iOS file protection.

import 'dart:async';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

const _channel = MethodChannel('dialog/security');

bool get _native => !kIsWeb && (Platform.isAndroid || Platform.isIOS);

// The user's setting, and how many screens currently force screen security on.
bool _settingOn = false;
int _holds = 0;

/// Turns screen security on or off (setting "Block screenshots", on by default). While a
/// [holdScreenSecurity] is active it stays on regardless.
Future<void> applyScreenSecurity(bool on) async {
  _settingOn = on;
  await _pushScreenSecurity();
}

Future<void> _pushScreenSecurity() async {
  if (!_native) return;
  try {
    await _channel.invokeMethod<void>('setScreenSecurity', _settingOn || _holds > 0);
  } catch (_) {}
}

/// Forces screen security on (view-once viewer) until the returned function is called, which
/// restores the user's setting. Calling it more than once is harmless.
Future<void> Function() holdScreenSecurity() {
  _holds++;
  unawaited(_pushScreenSecurity());
  var released = false;
  return () async {
    if (released) return;
    released = true;
    _holds--;
    await _pushScreenSecurity();
  };
}

/// How long a sensitive value stays on the clipboard.
const sensitiveClipboardSeconds = 60;

Timer? _clearTimer;

/// Copies a sensitive value (device link code, ...): marked sensitive so the system does not
/// preview it (Android 13+), kept on this device only (iOS, no Universal Clipboard) and cleared
/// after [sensitiveClipboardSeconds] if it is still there. Regular copies use [Clipboard].
/// On Android the clearing is a native AlarmManager alarm (survives backgrounding and process
/// death, see SensitiveClipboard.kt); the Dart timer below is only the fallback elsewhere.
Future<void> copySensitive(String text) async {
  if (_native) {
    try {
      await _channel.invokeMethod<void>('copySensitive', {'text': text, 'seconds': sensitiveClipboardSeconds});
      return;
    } catch (_) {}
  }
  await Clipboard.setData(ClipboardData(text: text));
  _clearTimer?.cancel();
  _clearTimer = Timer(const Duration(seconds: sensitiveClipboardSeconds), () async {
    try {
      final d = await Clipboard.getData(Clipboard.kTextPlain);
      if (d?.text == text) await Clipboard.setData(const ClipboardData(text: ''));
    } catch (_) {}
  });
}

/// iOS data protection class of a directory (new files inherit it):
/// "complete" | "completeUnlessOpen" | "untilFirstAuth".
Future<void> protectDirectory(String path, String level) async {
  if (kIsWeb || !Platform.isIOS) return;
  try {
    await _channel.invokeMethod<void>('protectDirectory', {'path': path, 'level': level});
  } catch (_) {}
}

/// Android: lets the app show over the lock screen (and turn the screen on) only while an
/// incoming call rings or a call is in progress. MainActivity turns it on when it is opened
/// from an incoming-call notification; it is off otherwise, so the app never appears over the
/// lock screen by itself.
Future<void> setShowOverLockScreen(bool on) async {
  if (kIsWeb || !Platform.isAndroid) return;
  try {
    await _channel.invokeMethod<void>('setShowWhenLocked', on);
  } catch (_) {}
}
