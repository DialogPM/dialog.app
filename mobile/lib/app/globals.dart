// Process-wide objects created at startup (see main.dart).

import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../core/backend.dart';
import '../state/prefs.dart';
import '../state/store.dart';

/// Built-in default server. Override at build time:
/// `flutter build apk --dart-define=CHATAPP_DEFAULT_SERVER=https://chat.example.org`
const String kDefaultServer = String.fromEnvironment('CHATAPP_DEFAULT_SERVER', defaultValue: 'https://chat.dialog.pm');

const String kAppVersion = '1.0.0';

late ProviderContainer container;
late Backend backend;

/// App-private directory (database, media, preferences).
late String dataDir;

final navigatorKey = GlobalKey<NavigatorState>();
final messengerKey = GlobalKey<ScaffoldMessengerState>();

AppStore get store => container.read(storeProvider.notifier);
AppData get appData => container.read(storeProvider);
Prefs get prefs => container.read(prefsProvider);
PrefsNotifier get prefsNotifier => container.read(prefsProvider.notifier);

/// Whether the keyboard may learn from what is typed (TextField.enableIMEPersonalizedLearning);
/// false with the "Incognito keyboard" setting (on by default, honored by Android keyboards).
bool get imePersonalizedLearning {
  try {
    return !prefs.incognitoKeyboard;
  } catch (_) {
    return false; // before startup (tests)
  }
}

bool get isMobilePlatform => !kIsWeb && (Platform.isAndroid || Platform.isIOS);

/// Platform name for the core: "android" | "ios" | "desktop".
String get corePlatform => kIsWeb
    ? 'web'
    : Platform.isAndroid
        ? 'android'
        : Platform.isIOS
            ? 'ios'
            : 'desktop';

/// Default name of this device when it joins an account.
String defaultDeviceName() {
  if (kIsWeb) return 'Web';
  if (Platform.isAndroid) return 'Android phone';
  if (Platform.isIOS) return 'iPhone';
  if (Platform.isMacOS) return 'Mac';
  if (Platform.isWindows) return 'Windows PC';
  if (Platform.isLinux) return 'Linux desktop';
  return 'Desktop';
}
