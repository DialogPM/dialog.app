// Remote wipe (crates/chat-core/src/revocation.rs): asks the server, for every account saved on
// this phone, whether this phone was removed from it, and deletes those accounts' data, even
// while they are locked. Runs at startup, every few minutes and when the app comes back. The
// active account's screens go to the start screen through the "logged_out" event; accounts
// removed from the start screen are reported here.

import 'dart:async';

import 'package:flutter/foundation.dart';

import '../app/globals.dart';
import '../l10n/i18n.dart';
import '../widgets/toast.dart';

/// Bumped when saved accounts were wiped (the start screen reloads its list).
final remoteWipeVersion = ValueNotifier<int>(0);

const _every = Duration(minutes: 10);
const _minGap = Duration(minutes: 1);

Timer? _timer;
DateTime? _last;
bool _running = false;

Future<void> checkRemoteWipe() async {
  final now = DateTime.now();
  if (_running || (_last != null && now.difference(_last!) < _minGap)) return;
  _running = true;
  _last = now;
  try {
    final wiped = await backend.checkRevoked();
    if (wiped.isNotEmpty) {
      remoteWipeVersion.value++;
      showToast(tr('mobile.remoteWiped', count: wiped.length));
    }
  } catch (e) {
    debugPrint('remote wipe check: $e');
  } finally {
    _running = false;
  }
}

void startRemoteWipeWatcher() {
  unawaited(checkRemoteWipe());
  _timer ??= Timer.periodic(_every, (_) => unawaited(checkRemoteWipe()));
}
