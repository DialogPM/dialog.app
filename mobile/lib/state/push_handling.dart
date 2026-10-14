// What the running app does with a push (services/push.dart, services/callkit.dart). A push
// only says "new message" or "incoming call" for one of the accounts on this phone (its push
// tag); the app fetches and decrypts the rest itself.
//
// - The open account: the core connects (or checks its socket) right away; the messages then
//   arrive and notify as usual, honoring previews, muted chats and do-not-disturb (events.dart),
//   and offers ring as usual (calls.dart).
// - Another account on this phone (only one is open at a time, chat-core profiles.rs): a
//   generic "New message in another account"; tapping it opens the accounts on this phone.
// - No account open (start screen): a generic "New message" / "Incoming call".

import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/foundation.dart';

import '../app/globals.dart';
import '../l10n/i18n.dart';
import '../services/callkit.dart';
import '../services/notifications.dart';
import '../services/push.dart';
import 'call_settings.dart' show callsAllowed;
import 'calls.dart';
import 'group_calls.dart' show groupCallKitId, groupCallState, handleGroupCallKitAction, setGroupMuted;
import 'events.dart' show appInForeground;
import 'profiles.dart' show switchAccount;

/// Push tag of the open account (null while none is open or unknown).
String? _activeTag;

bool get _sessionOpen => appData.appState?.ready == true;

PushPrefs _prefs() => PushPrefs(language: currentLanguage, notifications: prefs.notifications, dndUntil: prefs.dndUntil);

/// A push for the account the push tag [tag] belongs to: is it another account than the open one?
bool _otherAccount(String? tag) => tag != null && tag.isNotEmpty && _activeTag != null && tag != _activeTag;

/// Wires the push and CallKit callbacks (main.dart).
void initPushHandling() {
  Push.instance.onData = (data, {required background}) => unawaited(_onData(data, background: background));
  Push.instance.onOpened = _onOpened;
  Notifications.instance.onCallAction = (action, callId) => unawaited(handleCallAction(action, callId));
  Notifications.instance.onOpenProfiles = _openProfiles;
  CallKit.onIncoming = (callId, tag) => unawaited(_onCallKitIncoming(callId, tag));
  // Group calls are known to CallKit as "g:<conversation id>" (group_calls.dart).
  CallKit.onAnswer = (callId) =>
      unawaited(callId.startsWith('g:') ? handleGroupCallKitAction('answer', callId) : handleCallAction('answer', callId));
  CallKit.onEnd = (callId) =>
      unawaited(callId.startsWith('g:') ? handleGroupCallKitAction('end', callId) : handleCallAction('decline', callId));
  CallKit.onMute = (callId, muted) {
    if (callId.startsWith('g:')) {
      if (groupCallState.value != null && groupCallKitId(groupCallState.value!.conversationId) == callId) setGroupMuted(muted);
    } else if (callState.value?.callId == callId) {
      setCallMuted(muted);
    }
  };
}

/// After an account was opened: registers its push tokens and applies what happened before.
Future<void> onPushSessionStarted() async {
  try {
    _activeTag = await backend.pushTag();
  } catch (_) {
    _activeTag = null;
  }
  unawaited(Push.instance.register((provider, token) => backend.setPushToken(provider: provider, token: token)));
  await _applyStoredCallActions();
  final n = Notifications.instance;
  final launchCall = n.launchCallAction;
  if (launchCall != null) {
    n.launchCallAction = null;
    unawaited(handleCallAction(launchCall.$1, launchCall.$2));
  }
  final launch = Push.instance.takeLaunchData();
  if (launch != null) _onOpened(launch);
}

/// The open account is about to change (switch account, log out).
void onPushSessionEnded() => _activeTag = null;

Future<void> _onData(Map<String, dynamic> data, {required bool background}) async {
  final type = data['type'];
  if (type == 'call_action') {
    await handleCallAction(data['action'] as String? ?? '', data['call_id'] as String? ?? '');
    return;
  }
  final tag = data['p'] as String?;
  if (!_sessionOpen) {
    // Start screen: nothing can be fetched before an account is opened with its password.
    if (background || !appInForeground) await showGenericPush(data, _prefs());
    return;
  }
  if (_otherAccount(tag)) {
    await showGenericPush(data, _prefs(), otherAccount: true);
    return;
  }
  try {
    await backend.pushWake();
  } catch (e) {
    debugPrint('push wake: $e');
  }
  if (type == 'call') expectCall(data['call_id'] as String? ?? '');
}

void _onOpened(Map<String, dynamic> data) {
  if (_sessionOpen && _otherAccount(data['p'] as String?)) _openProfiles();
}

/// Opens the start screen with the accounts on this phone.
void _openProfiles() {
  if (_sessionOpen) unawaited(switchAccount());
}

/// A VoIP push was reported to CallKit (iOS; the native code already shows the call).
Future<void> _onCallKitIncoming(String callId, String? tag) async {
  if (!_sessionOpen) {
    // A cold start opens on the start screen: the account must be opened before answering.
    await CallKit.update(callId, name: tr('mobile.pushOpenToAnswer'));
    return;
  }
  if (_otherAccount(tag)) {
    markOtherAccountCall(callId);
    await CallKit.update(callId, name: tr('mobile.pushOtherAccountCall'));
    return;
  }
  // Rings until the offer arrives; if it never does (the caller hung up first, or the offer
  // came too late), the system call screen ends instead of ringing on.
  watchSystemCall(callId, const Duration(seconds: 20));
  try {
    // Fetches the offer (with calls turned off, the core answers the caller instead).
    await backend.pushWake();
  } catch (_) {}
  // Calls are turned off: the offer never reaches us, so the system call screen must not keep
  // ringing.
  if (!callsAllowed()) await CallKit.end(callId);
}

/// Declines decided while the app was not running (notification action), once the account
/// is open; entries older than the ring time are dropped.
Future<void> _applyStoredCallActions() async {
  try {
    final f = File('$dataDir/$kCallActionsFile');
    if (!f.existsSync()) return;
    final m = jsonDecode(f.readAsStringSync());
    f.deleteSync();
    if (m is! Map) return;
    final now = DateTime.now().millisecondsSinceEpoch;
    m.forEach((id, v) {
      if (id is String && v is Map && v['action'] == 'decline' && v['at'] is int && now - (v['at'] as int) < 120000) {
        unawaited(handleCallAction('decline', id));
      }
    });
  } catch (_) {}
}
