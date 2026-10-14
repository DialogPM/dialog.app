// Dialog mobile: startup. Initializes the Rust core (flutter_rust_bridge), the encrypted
// database key, preferences, notifications and optional push, then runs the app.

import 'dart:async';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_secure_storage/flutter_secure_storage.dart';
import 'package:intl/date_symbol_data_local.dart';
import 'package:path_provider/path_provider.dart';

import 'app/app.dart';
import 'services/remote_wipe.dart';
import 'app/globals.dart';
import 'core/backend.dart';
import 'l10n/i18n.dart';
import 'services/media.dart';
import 'services/notifications.dart';
import 'services/push.dart';
import 'services/security.dart';
import 'src/rust/api/core.dart' as core;
import 'src/rust/frb_generated.dart';
import 'state/actions.dart';
import 'state/call_system.dart';
import 'state/events.dart';
import 'state/prefs.dart';
import 'state/push_handling.dart';
import 'app/nav.dart';

const _dbKeyName = 'chatapp_db_key';

/// Returns the key the core starts with (never stored; see below).
Future<String> loadOrCreateDbKey(String dir) async {
  // No key is kept by the OS (keystore or file): the local data is written to disk only once
  // locked with the account password (chat-core local_lock.rs), so a per-run key is enough.
  // Keys stored by older versions are removed.
  try {
    await const FlutterSecureStorage().delete(key: _dbKeyName);
  } catch (_) {}
  try {
    final old = File('$dir/.db_key');
    if (old.existsSync()) old.deleteSync();
  } catch (_) {}
  return core.newDbKey();
}

Future<void> main() async {
  // Release builds write nothing to the system log (logcat / os_log can be read by other tools).
  if (kReleaseMode) debugPrint = (String? message, {int? wrapWidth}) {};
  WidgetsFlutterBinding.ensureInitialized();
  container = ProviderContainer();
  Object? startError;
  try {
    await RustLib.init();
    final dir = await getApplicationSupportDirectory();
    dir.createSync(recursive: true);
    dataDir = dir.path;
    // iOS data protection: the database and media are encrypted by the core with a key derived
    // from the password; they stay readable after the first unlock since calls (audio background
    // mode) and push keep the app working while the screen is locked.
    unawaited(protectDirectory(dataDir, 'untilFirstAuth'));
    prefsFilePath = '$dataDir/prefs.json';
    currentLanguage = container.read(prefsProvider).language ?? systemLanguage();
    unawaited(applyScreenSecurity(container.read(prefsProvider).screenSecurity));
    await Future.wait([initializeDateFormatting('en_US'), initializeDateFormatting('ro'), initializeDateFormatting('it')]);

    // Temporary plaintext files (picked copies, decrypted copies, exports) never outlive a run.
    unawaited(wipeAppTemp());
    final key = await loadOrCreateDbKey(dataDir);
    // Subscribe to events before the core starts so that nothing is missed. The stream stays
    // the same across account switches: only the active profile's core forwards its events.
    backend = Backend((cmd, req) => core.invoke(cmd: cmd, reqJson: req), core.events());
    startEventBridge();
    // Starts on the start screen (a blank profile). The accounts saved on this phone live in
    // dataDir/profiles/<id>/; an account of a version before profiles stays in dataDir itself
    // as the profile "default" (chat-core profiles.rs, state/profiles.dart).
    await core.startCore(
      config: core.BridgeConfig(dataDir: dataDir, dbKeyHex: key, defaultServerUrl: kDefaultServer, platform: corePlatform),
    );
  } catch (e, st) {
    debugPrint('startup failed: $e\n$st');
    startError = e;
  }

  if (startError == null) {
    await Notifications.instance.init(channelName: tr('settings.notifications.title'), callChannelName: tr('call.incomingTitle'));
    Notifications.instance.onOpen = openChatFromNotification;
    // Pushes, notification actions and CallKit (no-ops without Firebase / VoIP credentials).
    initPushHandling();
    // Android: foreground service + proximity lock while a call is in progress.
    initCallSystem();
    await Push.instance.init();
    onSessionStarted = () {
      unawaited(Notifications.instance.requestPermission());
      unawaited(onPushSessionStarted());
      final launch = Notifications.instance.launchConversation;
      if (launch != null) {
        Notifications.instance.launchConversation = null;
        Future<void>.delayed(const Duration(milliseconds: 300), () => openChatFromNotification(launch));
      }
    };
  }

  runApp(UncontrolledProviderScope(container: container, child: ChatApp(startError: startError)));
  if (startError == null) {
    unawaited(bootstrap());
    startRemoteWipeWatcher();
  }
}
