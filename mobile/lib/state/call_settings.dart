// Call privacy settings (port of the desktop's call settings): "Allow calls" turns every call of
// this account off (the core does not ring and tells callers we do not accept calls), "relay
// only" sends every call through the TURN relay so no peer learns our IP address. The settings
// live in the core and are synced to our other devices; callSettings mirrors them.

import 'package:flutter/foundation.dart';

import '../app/globals.dart';
import '../core/backend.dart';
import '../l10n/i18n.dart';
import '../widgets/toast.dart';

final callSettings = ValueNotifier<CallSettings>(CallSettings.defaults);

void applyCallSettings(CallSettings s) => callSettings.value = s;

bool callsAllowed() => callSettings.value.allowCalls;

bool relayOnlyCalls() => callSettings.value.relayOnly;

Future<void> loadCallSettings() async {
  try {
    applyCallSettings(await backend.getCallSettings());
  } catch (_) {
    // Older core: the defaults (calls on, direct allowed).
    applyCallSettings(CallSettings.defaults);
  }
}

Future<void> setCallSettings({bool? allowCalls, bool? relayOnly}) async =>
    applyCallSettings(await backend.setCallSettings(allowCalls: allowCalls, relayOnly: relayOnly));

/// Calls are off: tells the user where to turn them on. Returns false when calls are allowed.
bool blockedByCallSettings() {
  if (callsAllowed()) return false;
  showToast(tr('call.callsOffHint'));
  return true;
}
