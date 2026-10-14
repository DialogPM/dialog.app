// Battery: while the app is in the background and not in a call, the core closes its server
// connection (whose ping every 25 s keeps the phone's radio awake) and pushes bring new messages
// and calls (chat-core engine/net.rs set_background). During a call the connection stays open,
// in the background too: the call's signaling goes through it.

import 'dart:async';

import '../app/globals.dart';
import 'calls.dart';
import 'group_calls.dart';

bool _hidden = false;
bool? _sent;

/// Follows calls starting and ending (called once at startup).
void initBackgroundNetworking() {
  callState.addListener(_apply);
  groupCallState.addListener(_apply);
}

/// The app was hidden (true) or shown again (false).
void setAppHidden(bool hidden) {
  _hidden = hidden;
  _apply();
  // Presence: hidden means the user is not looking at the app, so they do not show online
  // (also during a call in the background).
  try {
    unawaited(backend.setPresenceActive(!hidden).catchError((Object _) {}));
  } catch (_) {}
}

void _apply() {
  final inCall = callState.value != null || groupCallState.value != null;
  final background = _hidden && !inCall;
  if (background == _sent) return;
  _sent = background;
  try {
    // No account open (locked, signed out): nothing to suspend; retried at the next change.
    unawaited(backend.setBackground(background).catchError((Object _) => _sent = null));
  } catch (_) {
    _sent = null;
  }
}
