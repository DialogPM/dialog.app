// iOS calls through CallKit (native side: ios/Runner/AppDelegate.swift, VoipCalls).
//
// - Incoming calls from a VoIP push (PushKit) are reported to CallKit by the native code
//   before Dart even runs (an iOS requirement).
// - Every other call is reported from Dart: outgoing calls ([startCall]), incoming calls that
//   reached the running app without a push ([reportIncoming], deduplicated by call id with the
//   push path), and group calls (ids "g:<conversation id>", see state/call_system.dart).
//
// A call CallKit knows keeps the app running while the screen is locked, and CallKit owns the
// audio session (WebRTC follows it, native side). This bridge tells the call logic
// (state/calls.dart, state/group_calls.dart) what the user did on the system call screen
// (answer, end, mute), and keeps that screen in step with the call.
//
// Every method is a no-op off iOS or when the native side does not answer (then the call goes
// on without CallKit, as before).

import 'dart:async';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

class CallKit {
  CallKit._();

  static const _channel = MethodChannel('dialog/voip');

  /// Call ids currently shown by CallKit.
  static final known = <String>{};

  /// A VoIP push was reported to CallKit (`tag`: the push tag of the account it is for).
  static void Function(String callId, String? tag)? onIncoming;

  /// The user answered / ended (declined or hung up) a call on the system call screen.
  static void Function(String callId)? onAnswer;
  static void Function(String callId)? onEnd;
  /// Mute (or hold) changed on the system call screen.
  static void Function(String callId, bool muted)? onMute;

  /// The PushKit token changed.
  static void Function(String token)? onToken;

  static bool get supported => !kIsWeb && Platform.isIOS;

  /// Starts receiving events; the native side then replays what happened before Dart ran.
  static Future<void> init() async {
    if (!supported) return;
    _channel.setMethodCallHandler(_handle);
    await _invoke('ready');
  }

  static Future<dynamic> _handle(MethodCall call) async {
    final args = call.arguments is Map ? Map<String, dynamic>.from(call.arguments as Map) : const <String, dynamic>{};
    final callId = args['call_id'] as String? ?? '';
    switch (call.method) {
      case 'voipToken':
        final token = args['token'] as String? ?? '';
        if (token.isNotEmpty) onToken?.call(token);
      case 'incoming':
        if (callId.isEmpty) return;
        known.add(callId);
        onIncoming?.call(callId, args['p'] as String?);
      case 'answer':
        if (callId.isNotEmpty) onAnswer?.call(callId);
      case 'end':
        if (callId.isEmpty) return;
        known.remove(callId);
        onEnd?.call(callId);
      case 'mute':
        if (callId.isNotEmpty) onMute?.call(callId, args['muted'] == true);
    }
    return null;
  }

  static Future<T?> _invoke<T>(String method, [Object? args]) async {
    if (!supported) return null;
    try {
      return await _channel.invokeMethod<T>(method, args);
    } catch (e) {
      debugPrint('voip $method: $e');
      return null;
    }
  }

  /// The current PushKit token (null before iOS delivered one, or off iOS).
  static Future<String?> voipToken() => _invoke<String>('getVoipToken');

  /// Shows [name] (or keeps "Dialog") and whether it is a video call on the system screen.
  static Future<void> update(String callId, {required String name, bool video = false}) async {
    if (!known.contains(callId)) return;
    await _invoke('updateCall', {'call_id': callId, 'name': name, 'video': video});
  }

  /// The call was answered in the app: the system screen follows.
  static Future<void> answered(String callId) async {
    if (!known.contains(callId)) return;
    await _invoke('answerCall', {'call_id': callId});
  }

  /// An outgoing call placed in the app. Shows [name] ("Dialog" unless notification previews
  /// are on). False when CallKit refused it (the call goes on without it).
  static Future<bool> startCall(String callId, {required String name, bool video = false}) async {
    if (!supported || callId.isEmpty) return false;
    known.add(callId);
    final ok = await _invoke<bool>('startCall', {'call_id': callId, 'name': name, 'video': video}) ?? false;
    if (!ok) known.remove(callId);
    return ok;
  }

  /// An incoming call that reached the running app (a call from a VoIP push is only updated).
  /// False when CallKit does not show it: the app then rings by itself.
  static Future<bool> reportIncoming(String callId, {required String name, bool video = false}) async {
    if (!supported || callId.isEmpty) return false;
    final already = known.contains(callId);
    known.add(callId);
    final ok = await _invoke<bool>('reportIncoming', {'call_id': callId, 'name': name, 'video': video}) ?? false;
    if (!ok && !already) known.remove(callId);
    return ok;
  }

  /// The media of the call is connected (an outgoing call shows as connected).
  static Future<void> connected(String callId) async {
    if (!known.contains(callId)) return;
    await _invoke('callConnected', {'call_id': callId});
  }

  /// Mute changed in the app: the system call screen follows.
  static Future<void> setMuted(String callId, bool muted) async {
    if (!known.contains(callId)) return;
    await _invoke('setMuted', {'call_id': callId, 'muted': muted});
  }

  /// The call ended (any reason) or was declined in the app. [local]: ended by the user here
  /// (an end-call action for the system), otherwise reported as ended by the other side, or as
  /// [failed].
  static Future<void> end(String callId, {bool local = false, bool failed = false}) async {
    if (!known.remove(callId)) return;
    await _invoke('endCall', {'call_id': callId, 'local': local, 'failed': failed});
  }
}
