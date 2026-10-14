// Accounts saved on this phone (local profiles, shared with the desktop app: chat-core
// profiles.rs). Each account has its own data directory; one is active at a time. The app
// starts on a blank profile (the start screen lists the saved accounts); opening one with its
// password, or "Switch account", swaps the core under the app (mobile/rust/src/api/core.rs),
// so the session state here is rebuilt from scratch afterwards.
//
// Only the active account is connected: messages, calls and notifications come from it alone.
// The other accounts stay logged in but offline until opened (their messages wait on the
// server).

import 'dart:async';

import '../app/globals.dart';
import '../core/backend.dart' show parseError;
import '../core/models.dart';
import '../services/notifications.dart';
import 'actions.dart';
import 'calls.dart';
import 'dnd.dart';
import 'group_calls.dart';
import 'push_handling.dart' show onPushSessionEnded;
import 'store.dart';

/// Name shown for a saved account ([fallback] when it has none, e.g. "Account").
String profileName(ProfileCard p, String fallback) {
  final label = p.label?.trim() ?? '';
  return label.isNotEmpty ? label : fallback;
}

/// "•••• 1234 · chat.example.com" (empty for a profile with neither).
String profileSubtitle(ProfileCard p) => [
      if (p.last4 != null && p.last4!.isNotEmpty) '•••• ${p.last4}',
      if (p.serverHost != null && p.serverHost!.isNotEmpty) p.serverHost!,
    ].join(' · ');

/// The profile whose password field is open when the start screen appears: the only one.
String? initiallyOpenProfile(List<ProfileCard> profiles) => profiles.length == 1 && !profiles.first.unlocked ? profiles.first.id : null;

/// True when the device list holds no device other than this one: logging out would leave the
/// account with no device at all (nobody can reach it until it logs in again).
bool isOnlyDevice(List<DeviceInfo> devices) => !devices.any((d) => !d.current);

/// Ends what belongs to the session on screen before its core goes away: calls (the peers are
/// told), group calls and the notifications of that account.
Future<void> _endSessionUi() async {
  onPushSessionEnded();
  try {
    await hangUp();
  } catch (_) {}
  try {
    await leaveGroupCall();
  } catch (_) {}
  groupCallsOngoing.value = const {};
  await Notifications.instance.cancelAll();
}

/// Rebuilds the app state after the active profile changed. [opened]: the user just entered
/// the password, so the app lock does not ask for it again.
Future<void> reloadAfterProfileSwitch({required bool opened}) async {
  // Drops every page and value of the previous session (the root shows the splash meanwhile).
  store.set((_) => const AppData());
  applyDnd(0);
  try {
    final s = await backend.getState();
    store.set((d) => d.copyWith(appState: s));
    if (s.ready) {
      initAppLock(s.account?.accountId, atStartup: !opened);
      await loadSession();
    }
  } catch (e) {
    notifyError(e);
    if (appData.appState == null) {
      store.set((d) => d.copyWith(appState: const AppState(status: 'no_account', defaultServerUrl: kDefaultServer)));
    }
  }
}

/// Opens a saved account with its password (throws on a wrong one; nothing changes then).
Future<void> openProfile(String id, String password) async {
  await backend.openProfile(id: id, password: password);
  await reloadAfterProfileSwitch(opened: true);
}

/// Quick switch to an account opened with its password since the app started. False when it
/// needs its password (then nothing changed).
Future<bool> quickSwitch(ProfileCard p) async {
  if (p.active) return true;
  if (!p.unlocked) return false;
  // Calls and notifications of the account on screen end first, as with "Switch account".
  await _endSessionUi();
  try {
    await backend.switchProfile(p.id);
  } catch (e) {
    if (parseError(e).code == 'password_required') return false;
    notifyError(e);
    return true;
  }
  await reloadAfterProfileSwitch(opened: true);
  return true;
}

/// Opens a saved account (also a hidden one) by its full number and password.
Future<void> openProfileByNumber(String number, String password) async {
  await backend.openProfileByNumber(number: number, password: password);
  await reloadAfterProfileSwitch(opened: true);
}

/// "Switch account": locks this account (it stays on this phone, logged in, with its data)
/// and goes back to the start screen.
Future<void> switchAccount() async {
  await attempt(() async {
    await _endSessionUi();
    await backend.closeProfile();
    await reloadAfterProfileSwitch(opened: false);
  });
}
