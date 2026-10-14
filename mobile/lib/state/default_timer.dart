// Default disappearing timer (port of the desktop's lib/defaultTimer.ts): chats the user starts
// and groups they create get it from the start; existing chats and chats started by others keep
// their own. The setting lives in the core (engine/default_timer.rs) and is synced to our other
// devices; defaultTimer mirrors it (seconds, 0 = off).

import 'package:flutter/foundation.dart';

import '../app/globals.dart';

final defaultTimer = ValueNotifier<int>(0);

void applyDefaultTimer(int seconds) => defaultTimer.value = seconds;

Future<void> loadDefaultTimer() async {
  try {
    applyDefaultTimer(await backend.getDefaultDisappearing());
  } catch (_) {
    // Older core: off.
    applyDefaultTimer(0);
  }
}

Future<void> setDefaultTimer(int seconds) async => applyDefaultTimer(await backend.setDefaultDisappearing(seconds));
