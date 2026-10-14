// One-to-one voice and video calls (port of app/src/store/calls.ts): WebRTC media through
// flutter_webrtc, signaling through the core (end-to-end encrypted over the Signal sessions, see
// crates/chat-core/src/engine/calls.rs). Interoperates with the desktop app.
//
// Caller: offer to every device of the peer -> devices ring and reply "ringing" -> the first
// device that answers gets the call (its device id becomes the target of every later signal)
// and tells its sibling devices to stop ringing. ICE candidates are trickled in batches.
//
// Video: the offer carries `video: true`; both sides send a camera track on the same peer
// connection. Turning the camera off only disables the track and tells the peer with a "media"
// signal, so it shows the avatar instead of black frames.

import 'dart:async';
import 'dart:io';
import 'dart:math';

import 'package:flutter/foundation.dart';
import 'package:flutter_webrtc/flutter_webrtc.dart';

import '../app/globals.dart';
import '../core/backend.dart';
import '../l10n/i18n.dart';
import '../services/call_sounds.dart';
import '../services/callkit.dart';
import '../services/notifications.dart';
import '../services/voice_fx.dart';
import '../services/security.dart';
import '../widgets/toast.dart';
import 'call_audio.dart';
import 'call_checks.dart';
import 'call_settings.dart';
import 'group_calls.dart' show groupCallState;
import 'dnd.dart' show dndActive;
import 'events.dart' show appInForeground;

/// Unanswered calls stop ringing after this long.
const _ringTimeout = Duration(seconds: 45);
/// A connection that stays down ("disconnected" / "failed") this long is given up. In between,
/// the caller restarts ICE (a "reoffer"): a network change, a locked phone that lost Wi-Fi
/// for a moment, etc. should not end the call.
const _disconnectGrace = Duration(seconds: 30);
/// "disconnected" often recovers by itself: the restart waits this long first.
const _restartDelay = Duration(seconds: 2);
/// At most one ICE restart this often while the connection is down.
const _restartEvery = Duration(seconds: 8);
const _iceBatch = Duration(milliseconds: 120);

enum CallPhase { outgoing, incoming, connecting, active }

@immutable
class CallState {
  final String callId;
  final String conversationId;
  final String peerAccountId;
  final String peerName;
  final String? peerAvatar;
  final bool outgoing;
  final CallPhase phase;
  /// Outgoing: the other side's device rings.
  final bool ringing;
  final bool muted;
  /// Video call (the caller asked for video).
  final bool video;
  /// Our camera is sending.
  final bool cameraOn;
  /// The peer's camera is sending.
  final bool remoteCameraOn;
  final bool speaker;
  final MediaStream? localStream;
  final MediaStream? remoteStream;
  final int startedAt;
  final int? connectedAt;
  /// Connection quality while active (call_audio.dart).
  final QualityLevel quality;
  final QualityWho? qualityWho;
  /// Our camera was stopped because the connection could not carry video.
  final bool autoCameraOff;
  /// Our live voice effect (services/voice_fx.dart), "none" at the start of every call.
  final String voiceEffect;
  /// AI noise reduction of our microphone (starts from the setting).
  final bool denoise;

  const CallState({
    required this.callId,
    required this.conversationId,
    required this.peerAccountId,
    required this.peerName,
    this.peerAvatar,
    required this.outgoing,
    required this.phase,
    this.ringing = false,
    this.muted = false,
    required this.video,
    required this.cameraOn,
    required this.remoteCameraOn,
    required this.speaker,
    this.localStream,
    this.remoteStream,
    required this.startedAt,
    this.connectedAt,
    this.quality = QualityLevel.good,
    this.qualityWho,
    this.autoCameraOff = false,
    this.voiceEffect = 'none',
    this.denoise = false,
  });

  bool get hasCamera => localStream?.getVideoTracks().isNotEmpty ?? false;

  CallState copyWith({
    CallPhase? phase,
    bool? ringing,
    bool? muted,
    bool? cameraOn,
    bool? remoteCameraOn,
    bool? speaker,
    MediaStream? localStream,
    MediaStream? remoteStream,
    int? connectedAt,
    QualityLevel? quality,
    QualityWho? Function()? qualityWho,
    bool? autoCameraOff,
    String? voiceEffect,
    bool? denoise,
  }) =>
      CallState(
        callId: callId,
        conversationId: conversationId,
        peerAccountId: peerAccountId,
        peerName: peerName,
        peerAvatar: peerAvatar,
        outgoing: outgoing,
        phase: phase ?? this.phase,
        ringing: ringing ?? this.ringing,
        muted: muted ?? this.muted,
        video: video,
        cameraOn: cameraOn ?? this.cameraOn,
        remoteCameraOn: remoteCameraOn ?? this.remoteCameraOn,
        speaker: speaker ?? this.speaker,
        localStream: localStream ?? this.localStream,
        remoteStream: remoteStream ?? this.remoteStream,
        startedAt: startedAt,
        connectedAt: connectedAt ?? this.connectedAt,
        quality: quality ?? this.quality,
        qualityWho: qualityWho != null ? qualityWho() : this.qualityWho,
        autoCameraOff: autoCameraOff ?? this.autoCameraOff,
        voiceEffect: voiceEffect ?? this.voiceEffect,
        denoise: denoise ?? this.denoise,
      );
}

/// The current call (null when there is none).
final callState = ValueNotifier<CallState?>(null);

void _set(CallState Function(CallState c) update) {
  final c = callState.value;
  if (c != null) callState.value = update(c);
}

// --- non-reactive session state -------------------------------------------------------------

class _Session {
  _Session(this.callId);

  final String callId;
  RTCPeerConnection? pc;
  MediaStream? stream;
  /// Device of the peer we talk to (known after the offer / answer).
  int? peerDevice;
  /// Remote offer of an incoming call, kept until accepted.
  String? offerSdp;
  bool remoteDescriptionSet = false;
  /// Remote candidates received before the remote description was set.
  final remoteIce = <RTCIceCandidate>[];
  /// Remote candidates accepted so far (capped at [maxRemoteIce] per call).
  int remoteIceCount = 0;
  /// Media only through our TURN relay: the peer must not learn our IP address.
  bool relay = false;
  /// Local candidates waiting to be sent (caller: until the answer names the device).
  final localIce = <Map<String, dynamic>>[];
  Timer? iceTimer;
  Timer? ringTimer;
  Timer? disconnectTimer;
  Timer? restartTimer;
  /// Caller: an ICE restart offer is waiting for its answer.
  bool restartPending = false;
  DateTime? lastRestart;
  void Function()? stopSound;
  bool ended = false;
  Timer? qualityTimer;
  final meter = QualityMeter();
  /// Consecutive samples with too little bandwidth for video.
  int lowBitrate = 0;
}

_Session? _session;

String _myAccountId() => appData.appState?.account?.accountId ?? '';

Future<void> _signal(String kind, [Object? data, bool toOwnDevices = false]) async {
  final call = callState.value;
  final s = _session;
  if (call == null || s == null) return;
  try {
    await backend.callSignal(
      accountId: call.peerAccountId,
      callId: call.callId,
      kind: kind,
      data: data,
      targetDevice: toOwnDevices ? null : s.peerDevice,
      toOwnDevices: toOwnDevices,
    );
  } catch (e) {
    debugPrint('call signal $kind failed: $e');
  }
}

void _stopSound() {
  _session?.stopSound?.call();
  _session?.stopSound = null;
}

Future<void> _playSound(_Session s, Future<void Function()> Function() start) async {
  final stop = await start();
  if (_session != s || s.ended) {
    stop();
  } else {
    s.stopSound = stop;
  }
}

Future<List<Map<String, dynamic>>> _iceServers() async {
  try {
    return await backend.getIceServers();
  } catch (e) {
    debugPrint('ICE servers unavailable: $e');
    return const [];
  }
}

const _audio = {'echoCancellation': true, 'noiseSuppression': true, 'autoGainControl': true};
const _video = {
  'facingMode': 'user',
  'mandatory': {'minWidth': '1280', 'minHeight': '720', 'minFrameRate': '30'},
  'optional': <Object>[],
};

/// Shared with group calls (group_calls.dart).
Future<MediaStream?> callLocalMedia(bool video) => _localMedia(video);
Future<List<Map<String, dynamic>>> callIceServers() => _iceServers();
bool callHasTurn(List<Map<String, dynamic>> servers) => _hasTurn(servers);

/// "Calls only through the Dialog server" is on but the server has no TURN relay: the call is
/// refused rather than connected directly. Shows why and returns true.
bool callRelayUnavailable(List<Map<String, dynamic>> servers) {
  if (!relayOnlyCalls() || _hasTurn(servers)) return false;
  showToast(tr('call.relayUnavailable'), error: true);
  return true;
}

/// Defense in depth when media must go through the relay: only relay candidates leave the
/// device (with the "relay" policy WebRTC gathers no others).
bool callCandidateAllowed(String? candidate, bool relayOnly) =>
    candidate != null && (!relayOnly || candidate.contains(' typ relay'));

/// Microphone, plus the camera for a video call. Without a camera the call goes on audio-only.
Future<MediaStream?> _localMedia(bool video) async {
  if (video) {
    try {
      return await navigator.mediaDevices.getUserMedia({'audio': _audio, 'video': _video});
    } catch (e) {
      debugPrint('camera unavailable for call: $e');
      showToast(tr('call.cameraError'));
    }
  }
  try {
    return await navigator.mediaDevices.getUserMedia({'audio': _audio, 'video': false});
  } catch (e) {
    debugPrint('microphone unavailable for call: $e');
    showToast(tr('voice.micError'), error: true);
    return null;
  }
}

void _flushLocalIce() {
  final s = _session;
  if (s == null || s.localIce.isEmpty) return;
  // The caller waits for the answer: until then it does not know which device to talk to.
  if (callState.value?.outgoing == true && s.peerDevice == null) return;
  final candidates = List<Map<String, dynamic>>.of(s.localIce);
  s.localIce.clear();
  unawaited(_signal('ice', {'candidates': candidates}));
}

bool _hasTurn(List<Map<String, dynamic>> servers) => servers.any((srv) {
      final urls = srv['urls'];
      final list = urls is List ? urls : [urls];
      return list.any((u) => u is String && (u.startsWith('turn:') || u.startsWith('turns:')));
    });

Future<RTCPeerConnection> _createPeerConnection(List<Map<String, dynamic>> servers, MediaStream stream) async {
  final s = _session!;
  // Calls with someone we have not accepted go through the relay only, so they cannot learn
  // our IP address. Without a TURN server such a call would never connect: it then goes direct.
  // With "Calls only through the Dialog server" every call is relayed (callers refuse the call
  // before getting here when there is no TURN server, see callRelayUnavailable).
  var policy = 'all';
  if (s.relay || relayOnlyCalls()) {
    if (_hasTurn(servers)) {
      policy = 'relay';
    } else {
      debugPrint('call: relay-only requested but no TURN server is configured; connecting directly');
    }
  }
  final pc = await createPeerConnection({'iceServers': servers, 'sdpSemantics': 'unified-plan', 'iceTransportPolicy': policy, ..._networkChange});
  for (final track in stream.getTracks()) {
    await pc.addTrack(track, stream);
  }
  // A video call without our own camera still receives the peer's video.
  if (callState.value?.video == true && stream.getVideoTracks().isEmpty) {
    await pc.addTransceiver(
      kind: RTCRtpMediaType.RTCRtpMediaTypeVideo,
      init: RTCRtpTransceiverInit(direction: TransceiverDirection.RecvOnly),
    );
  }
  pc.onIceCandidate = (c) {
    if (_session != s || !callCandidateAllowed(c.candidate, policy == 'relay')) return;
    s.localIce.add(c.toMap() as Map<String, dynamic>);
    s.iceTimer?.cancel();
    s.iceTimer = Timer(_iceBatch, _flushLocalIce);
  };
  pc.onTrack = (e) {
    if (_session != s || e.streams.isEmpty) return;
    final remote = e.streams.first;
    _set((c) => c.copyWith(remoteStream: remote));
  };
  pc.onConnectionState = (state) {
    if (_session != s) return;
    switch (state) {
      case RTCPeerConnectionState.RTCPeerConnectionStateConnected:
        s.disconnectTimer?.cancel();
        s.disconnectTimer = null;
        s.restartTimer?.cancel();
        s.restartTimer = null;
        final call = callState.value;
        if (call != null && call.phase != CallPhase.active) {
          _set((c) => c.copyWith(phase: CallPhase.active, connectedAt: c.connectedAt ?? DateTime.now().millisecondsSinceEpoch));
          unawaited(CallKit.connected(s.callId));
          // The audio session starts with the media: apply the chosen output again.
          unawaited(Helper.setSpeakerphoneOn(call.speaker).catchError((_) {}));
        }
        unawaited(prioritizeAudio(pc));
        s.qualityTimer ??= Timer.periodic(_qualityEvery, (_) => unawaited(_checkQuality(s)));
      case RTCPeerConnectionState.RTCPeerConnectionStateDisconnected:
        _connectionLost(s);
        s.restartTimer?.cancel();
        s.restartTimer = Timer(_restartDelay, () => unawaited(_restartIce(s)));
      case RTCPeerConnectionState.RTCPeerConnectionStateFailed:
        // One more chance (ICE restart) before the call is given up.
        _connectionLost(s);
        s.restartTimer?.cancel();
        s.restartTimer = null;
        unawaited(_restartIce(s));
      default:
        break;
    }
  };
  return pc;
}

/// The connection went down: the call ends unless it is back within [_disconnectGrace].
void _connectionLost(_Session s) {
  s.disconnectTimer ??= Timer(_disconnectGrace, () {
    if (_session == s) unawaited(_endCall('failed', notifyPeer: true));
  });
}

/// ICE restart, by the side that made the original offer (the caller), on the same peer
/// connection and call id: a new offer ("reoffer") to the device we talk to, answered with an
/// "answer". The callee waits for it. Retried while the connection stays down.
Future<void> _restartIce(_Session s) async {
  s.restartTimer = null;
  final call = callState.value;
  final pc = s.pc;
  if (_session != s || s.ended || call == null || !call.outgoing || pc == null || s.peerDevice == null || !s.remoteDescriptionSet && !s.restartPending) {
    return;
  }
  if (call.phase != CallPhase.connecting && call.phase != CallPhase.active) return;
  final now = DateTime.now();
  final last = s.lastRestart;
  if (last != null && now.difference(last) < _restartEvery) {
    // Too soon after the last attempt: try again later while still down.
    s.restartTimer = Timer(_restartEvery - now.difference(last), () => unawaited(_restartIce(s)));
    return;
  }
  if (pc.connectionState == RTCPeerConnectionState.RTCPeerConnectionStateConnected) return;
  s.lastRestart = now;
  try {
    await pc.restartIce();
    final offer = tuned(await pc.createOffer({'iceRestart': true}));
    if (_session != s || s.ended) return;
    await pc.setLocalDescription(offer);
    if (_session != s || s.ended) return;
    // Candidates of the old generation are of no use any more; new ones wait for the answer.
    s.restartPending = true;
    s.remoteDescriptionSet = false;
    s.remoteIce.clear();
    s.remoteIceCount = 0;
    await _signal('reoffer', {'sdp': offer.sdp});
  } catch (e) {
    debugPrint('call: ICE restart failed: $e');
  }
  // Still down at the next slot: another attempt (the grace timer ends the call eventually).
  if (_session == s && !s.ended && s.disconnectTimer != null) {
    s.restartTimer?.cancel();
    s.restartTimer = Timer(_restartEvery, () => unawaited(_restartIce(s)));
  }
}

Future<void> _applyRemoteIce() async {
  final s = _session;
  final pc = s?.pc;
  if (s == null || pc == null || !s.remoteDescriptionSet) return;
  final pending = List<RTCIceCandidate>.of(s.remoteIce);
  s.remoteIce.clear();
  for (final c in pending) {
    try {
      await pc.addCandidate(c);
    } catch (e) {
      debugPrint('bad ICE candidate: $e');
    }
  }
}

/// [local]: ended by the user here; [failed]: the connection was lost (for CallKit).
void _cleanup({bool local = false, bool failed = false}) {
  final s = _session;
  if (s == null) return;
  s.ended = true;
  _stopSound();
  s.iceTimer?.cancel();
  s.ringTimer?.cancel();
  s.disconnectTimer?.cancel();
  s.restartTimer?.cancel();
  s.qualityTimer?.cancel();
  unawaited(VoiceFxNative.configure('none', false));
  final stream = s.stream;
  final pc = s.pc;
  _session = null;
  callState.value = null;
  // The ringing notification / system call screen go with the call.
  unawaited(Notifications.instance.cancelCall());
  unawaited(CallKit.end(s.callId, local: local, failed: failed));
  unawaited(setShowOverLockScreen(false));
  // Released after the call screen has dropped its renderers (next frames).
  Future<void>.delayed(const Duration(milliseconds: 400), () async {
    for (final t in stream?.getTracks() ?? const <MediaStreamTrack>[]) {
      await t.stop().catchError((_) {});
    }
    await stream?.dispose().catchError((_) {});
    await pc?.close().catchError((_) {});
  });
}

/// Ends the current call, records it in the conversation and optionally tells the peer.
Future<void> _endCall(String status, {required bool notifyPeer, String? reason}) async {
  final call = callState.value;
  final s = _session;
  if (call == null || s == null || s.ended) return;
  if (notifyPeer) await _signal('hangup', {'reason': reason ?? status});
  final now = DateTime.now().millisecondsSinceEpoch;
  final duration = call.connectedAt != null ? now - call.connectedAt! : 0;
  final finalStatus = call.connectedAt != null ? 'answered' : status;
  _cleanup(local: notifyPeer && status != 'failed', failed: status == 'failed');
  try {
    await backend.recordCall(
      conversationId: call.conversationId,
      callId: call.callId,
      callerAccountId: call.outgoing ? _myAccountId() : call.peerAccountId,
      status: finalStatus,
      durationMs: duration,
      startedAt: call.startedAt,
    );
  } catch (e) {
    debugPrint('record call failed: $e');
  }
}

// --- public actions -------------------------------------------------------------------------

Future<void> startCall(String conversationId, {bool video = false}) async {
  if (callState.value != null || groupCallState.value != null) return;
  final conv = appData.conversations[conversationId];
  if (conv == null || conv.isGroup || conv.peerAccountId == null) return;
  if (blockedByCallSettings()) return;
  final callId = _uuid();
  // Not a contact, or "calls only through the Dialog server": media through the relay only
  // (same rule as the desktop app).
  final s = _Session(callId)..relay = !conv.peerIsContact || relayOnlyCalls();
  _session = s;
  callState.value = CallState(
    callId: callId,
    conversationId: conversationId,
    peerAccountId: conv.peerAccountId!,
    peerName: conv.title,
    peerAvatar: conv.avatarPath,
    outgoing: true,
    phase: CallPhase.outgoing,
    video: video,
    cameraOn: video,
    remoteCameraOn: video,
    speaker: video,
    startedAt: DateTime.now().millisecondsSinceEpoch,
  );
  // iOS: the system knows about the call (keeps it alive with the screen locked, owns audio).
  unawaited(CallKit.startCall(callId, name: prefs.notifPreview ? conv.title : kBrandName, video: video));
  final servers = await _iceServers();
  if (_session != s) return;
  if (callRelayUnavailable(servers)) {
    _cleanup();
    return;
  }
  final stream = await _localMedia(video);
  if (_session != s) {
    await _release(stream);
    return;
  }
  if (stream == null) {
    _cleanup();
    return;
  }
  s.stream = stream;
  await _startVoiceFx();
  _set((c) => c.copyWith(localStream: stream, cameraOn: stream.getVideoTracks().isNotEmpty));
  unawaited(Helper.setSpeakerphoneOn(video).catchError((_) {}));
  final pc = await _createPeerConnection(servers, stream);
  s.pc = pc;
  final offer = tuned(await pc.createOffer());
  await pc.setLocalDescription(offer);
  if (_session != s) return;
  await _signal('offer', {'sdp': offer.sdp, 'video': video});
  unawaited(_playSound(s, startRingback));
  s.ringTimer = Timer(_ringTimeout, () => unawaited(_endCall('missed', notifyPeer: true, reason: 'timeout')));
}

Future<void> acceptCall() async {
  final call = callState.value;
  final s = _session;
  final offer = s?.offerSdp;
  if (call == null || s == null || call.phase != CallPhase.incoming || offer == null) return;
  _stopSound();
  s.ringTimer?.cancel();
  _set((c) => c.copyWith(phase: CallPhase.connecting));
  unawaited(Notifications.instance.cancelCall());
  unawaited(CallKit.answered(call.callId));
  final servers = await _iceServers();
  if (_session != s) return;
  if (callRelayUnavailable(servers)) {
    await _endCall('failed', notifyPeer: true, reason: 'relay_unavailable');
    return;
  }
  final stream = await _localMedia(call.video);
  if (_session != s) {
    await _release(stream);
    return;
  }
  if (stream == null) {
    await _endCall('failed', notifyPeer: true, reason: 'no_microphone');
    return;
  }
  s.stream = stream;
  await _startVoiceFx();
  _set((c) => c.copyWith(localStream: stream, cameraOn: stream.getVideoTracks().isNotEmpty));
  unawaited(Helper.setSpeakerphoneOn(call.video).catchError((_) {}));
  final pc = await _createPeerConnection(servers, stream);
  s.pc = pc;
  await pc.setRemoteDescription(RTCSessionDescription(offer, 'offer'));
  s.remoteDescriptionSet = true;
  final answer = tuned(await pc.createAnswer());
  await pc.setLocalDescription(answer);
  if (_session != s) return;
  await _signal('answer', {'sdp': answer.sdp});
  if (call.video && stream.getVideoTracks().isEmpty) unawaited(_signal('media', {'camera': false}));
  // Our other devices stop ringing.
  unawaited(_signal('hangup', {'reason': 'answered_elsewhere'}, true));
  await _applyRemoteIce();
  _flushLocalIce();
}

Future<void> declineCall() async {
  final call = callState.value;
  if (call == null || call.phase != CallPhase.incoming) return;
  unawaited(_signal('hangup', {'reason': 'declined_elsewhere'}, true));
  await _endCall('declined', notifyPeer: true, reason: 'declined');
}

Future<void> hangUp() async {
  final call = callState.value;
  if (call == null) return;
  if (call.phase == CallPhase.incoming) return declineCall();
  await _endCall(call.outgoing && call.connectedAt == null ? 'cancelled' : 'answered', notifyPeer: true);
}

void toggleMute() {
  final call = callState.value;
  if (call != null) setCallMuted(!call.muted);
}

/// Mutes / unmutes the microphone (call screen, or the system call screen through CallKit).
void setCallMuted(bool muted) {
  final call = callState.value;
  final stream = _session?.stream;
  if (call == null || stream == null || call.muted == muted) return;
  for (final t in stream.getAudioTracks()) {
    t.enabled = !muted;
  }
  _set((c) => c.copyWith(muted: muted));
  unawaited(CallKit.setMuted(call.callId, muted));
}

const _qualityEvery = Duration(seconds: 2);

/// Updates the "weak connection" notice; in a video call, stops our camera when the
/// connection cannot carry it any more (6 s below [videoMinBitrate]), so the voice stays clear.
Future<void> _checkQuality(_Session s) async {
  final pc = s.pc;
  if (_session != s || s.ended || pc == null) return;
  final Quality q;
  try {
    q = await s.meter.sample(pc);
  } catch (_) {
    return;
  }
  final call = callState.value;
  if (_session != s || call == null) return;
  if (call.quality != q.level || call.qualityWho != q.who) _set((c) => c.copyWith(quality: q.level, qualityWho: () => q.who));
  final low = q.outBitrate != null && q.outBitrate! < videoMinBitrate && q.level != QualityLevel.good;
  s.lowBitrate = low ? s.lowBitrate + 1 : 0;
  if (call.video && call.cameraOn && s.lowBitrate >= 3) {
    s.lowBitrate = 0;
    toggleCamera();
    _set((c) => c.copyWith(autoCameraOff: true));
  }
}

/// A new call: a fresh effect processor, with the noise reduction setting applied.
Future<void> _startVoiceFx() async {
  await VoiceFxNative.newCall();
  final denoise = prefs.callDenoise && await VoiceFxNative.configure('none', true);
  if (!prefs.callDenoise) await VoiceFxNative.configure('none', false);
  _set((c) => c.copyWith(voiceEffect: 'none', denoise: denoise));
}

/// Switches our voice effect and/or the noise reduction during the call (the other side hears
/// it at once).
Future<void> setVoiceEffect({String? effect, bool? denoise}) async {
  final call = callState.value;
  if (call == null) return;
  final e = effect ?? call.voiceEffect;
  final d = denoise ?? call.denoise;
  if (await VoiceFxNative.configure(e, d)) {
    _set((c) => c.copyWith(voiceEffect: e, denoise: d));
  } else {
    showToast(tr('voiceFx.error'), error: true);
  }
}

void toggleCamera() {
  final call = callState.value;
  final s = _session;
  final tracks = s?.stream?.getVideoTracks() ?? const <MediaStreamTrack>[];
  if (call == null || s == null || tracks.isEmpty) return;
  final on = !call.cameraOn;
  tracks.first.enabled = on;
  _set((c) => c.copyWith(cameraOn: on, autoCameraOff: false));
  // Before the answer the peer device is unknown: the state is sent once it answers.
  if (s.peerDevice != null && call.phase != CallPhase.incoming) unawaited(_signal('media', {'camera': on}));
}

Future<void> switchCamera() async {
  final tracks = _session?.stream?.getVideoTracks() ?? const <MediaStreamTrack>[];
  if (tracks.isEmpty) return;
  try {
    await Helper.switchCamera(tracks.first);
  } catch (e) {
    debugPrint('switch camera: $e');
  }
}

Future<void> toggleSpeaker() async {
  final call = callState.value;
  if (call == null) return;
  final on = !call.speaker;
  _set((c) => c.copyWith(speaker: on));
  try {
    await Helper.setSpeakerphoneOn(on);
  } catch (e) {
    debugPrint('speakerphone: $e');
  }
}

Future<void> _release(MediaStream? stream) async {
  if (stream == null) return;
  for (final t in stream.getTracks()) {
    await t.stop();
  }
  await stream.dispose();
}

// --- incoming signals -----------------------------------------------------------------------

/// Well-formed remote candidates, within the per-call budget of [s].
List<RTCIceCandidate> _candidatesOf(_Session s, Map<String, dynamic>? data) {
  final list = data?['candidates'];
  if (list is! List) return const [];
  final out = <RTCIceCandidate>[];
  for (final raw in list) {
    if (s.remoteIceCount >= maxRemoteIce) break;
    final c = cleanCandidate(raw);
    if (c == null) continue;
    s.remoteIceCount++;
    out.add(RTCIceCandidate(c.candidate, c.sdpMid, c.sdpMLineIndex));
  }
  return out;
}

String? _sdpOf(Map<String, dynamic>? data) {
  final sdp = data?['sdp'];
  if (sdp is! String) return null;
  if (!validSdp(sdp)) {
    debugPrint('call: remote SDP rejected');
    return null;
  }
  return sdp;
}

Future<void> _recordMissed(CallSignalEvent ev) async {
  try {
    await backend.recordCall(
      conversationId: ev.conversationId!,
      callId: ev.callId,
      callerAccountId: ev.accountId,
      status: 'missed',
      durationMs: 0,
      startedAt: DateTime.now().millisecondsSinceEpoch,
    );
  } catch (_) {}
}

Future<void> handleCallSignal(CallSignalEvent ev) async {
  final call = callState.value;
  final s = _session;
  final current = call != null && s != null && call.callId == ev.callId ? call : null;

  if (ev.fromMe) {
    // Answered or declined on another of our devices: stop ringing here.
    if (current?.phase == CallPhase.incoming && (ev.kind == 'hangup' || ev.kind == 'answer')) _cleanup();
    if (current == null && (ev.kind == 'hangup' || ev.kind == 'answer')) _endOrphanSystemCall(ev.callId);
    return;
  }

  switch (ev.kind) {
    case 'offer':
      final sdp = _sdpOf(ev.data);
      if (sdp == null || ev.conversationId == null) return;
      // The same offer again (resent, or delivered twice): we are already handling it.
      if (call?.callId == ev.callId) return;
      // Glare: we are calling this peer while they call us. Both sides decide the same way:
      // the lower call id wins. The winner ignores the other offer (the peer switches to it);
      // the loser drops its own call silently and rings for the winner's.
      if (call != null && s != null && call.outgoing && call.phase == CallPhase.outgoing && call.peerAccountId == ev.accountId) {
        if (call.callId.compareTo(ev.callId) < 0) return;
        // Stops the peer's other devices from ringing for our call; the device that called us
        // ignores it (another call id).
        await _signal('hangup', {'reason': 'glare'});
        if (_session == s) _cleanup();
      }
      if (callState.value != null || groupCallState.value != null) {
        // Already in a call: tell the caller we are busy.
        try {
          await backend.callSignal(accountId: ev.accountId, callId: ev.callId, kind: 'busy', targetDevice: ev.deviceId);
        } catch (_) {}
        await _recordMissed(ev);
        return;
      }
      // Declined on the notification or the system call screen before the offer arrived.
      if (_pendingDecline.remove(ev.callId)) {
        try {
          await backend.callSignal(accountId: ev.accountId, callId: ev.callId, kind: 'hangup', data: {'reason': 'declined'}, targetDevice: ev.deviceId);
          // Our other devices stop ringing.
          await backend.callSignal(accountId: ev.accountId, callId: ev.callId, kind: 'hangup', data: {'reason': 'declined_elsewhere'}, toOwnDevices: true);
        } catch (_) {}
        unawaited(CallKit.end(ev.callId));
        await _recordMissed(ev);
        return;
      }
      final conv = appData.conversations[ev.conversationId];
      final contact = appData.contacts[ev.accountId];
      final video = ev.data?['video'] == true;
      final ns = _Session(ev.callId)
        ..peerDevice = ev.deviceId
        ..offerSdp = sdp
        ..relay = ev.relay;
      _session = ns;
      final peerName = conv?.title ?? contact?.displayName ?? '';
      callState.value = CallState(
        callId: ev.callId,
        conversationId: ev.conversationId!,
        peerAccountId: ev.accountId,
        peerName: peerName,
        peerAvatar: conv?.avatarPath ?? contact?.avatarPath,
        outgoing: false,
        phase: CallPhase.incoming,
        ringing: true,
        video: video,
        cameraOn: video,
        remoteCameraOn: video,
        speaker: video,
        startedAt: DateTime.now().millisecondsSinceEpoch,
      );
      unawaited(_signal('ringing'));
      ns.ringTimer = Timer(_ringTimeout, () => unawaited(_endCall('missed', notifyPeer: false)));
      _expectedCalls.remove(ev.callId)?.cancel();
      // "Notification preview" off: the lock screen must not show who is calling.
      final title = prefs.notifPreview ? tr(video ? 'call.incomingVideoTitle' : 'call.incomingTitle') : kBrandName;
      final body = prefs.notifPreview ? peerName : tr('call.incomingTitle');
      // iOS rings through CallKit: reported by the VoIP push already (then only updated), or
      // now. No second ringtone or notification then.
      final callKitName = prefs.notifPreview ? peerName : kBrandName;
      var viaCallKit = CallKit.known.contains(ev.callId);
      if (viaCallKit) {
        unawaited(CallKit.update(ev.callId, name: callKitName, video: video));
      } else {
        viaCallKit = await CallKit.reportIncoming(ev.callId, name: callKitName, video: video);
      }
      // Ended meanwhile: _cleanup took the call off CallKit too.
      if (_session != ns || ns.ended) return;
      if (!viaCallKit) {
        unawaited(_playSound(ns, startRingtone));
        if (!appInForeground) {
          unawaited(Platform.isAndroid
              ? Notifications.instance.showIncomingCall(
                  callId: ev.callId,
                  title: title,
                  body: body,
                  answerLabel: tr('call.accept'),
                  declineLabel: tr('call.decline'),
                )
              : Notifications.instance.showSimple(title, body));
        }
      }
      // Answered on the notification or the system call screen before the offer arrived.
      if (_pendingAnswer == ev.callId) {
        _pendingAnswer = null;
        unawaited(acceptCall());
      }
    case 'ringing':
      if (current?.outgoing == true) _set((c) => c.copyWith(ringing: true));
    case 'answer':
      if (current != null && current.outgoing && s != null && s.restartPending && ev.deviceId == s.peerDevice) {
        // Answer to our ICE restart.
        final sdp = _sdpOf(ev.data);
        final pc = s.pc;
        if (sdp == null || pc == null) return;
        s.restartPending = false;
        try {
          await pc.setRemoteDescription(RTCSessionDescription(sdp, 'answer'));
        } catch (e) {
          debugPrint('call: restart answer rejected: $e');
          return;
        }
        if (_session != s) return;
        s.remoteDescriptionSet = true;
        await _applyRemoteIce();
        _flushLocalIce();
        return;
      }
      if (current == null || !current.outgoing || current.phase != CallPhase.outgoing || s == null) return;
      final sdp = _sdpOf(ev.data);
      final pc = s.pc;
      if (sdp == null || pc == null) return;
      s.peerDevice = ev.deviceId;
      _stopSound();
      s.ringTimer?.cancel();
      _set((c) => c.copyWith(phase: CallPhase.connecting));
      if (current.video && callState.value?.cameraOn == false) unawaited(_signal('media', {'camera': false}));
      await pc.setRemoteDescription(RTCSessionDescription(sdp, 'answer'));
      s.remoteDescriptionSet = true;
      await _applyRemoteIce();
      _flushLocalIce();
    case 'reoffer':
      // The caller restarts ICE on the call in progress (same peer connection).
      if (current == null || s == null || current.outgoing || ev.deviceId != s.peerDevice) return;
      if (current.phase != CallPhase.connecting && current.phase != CallPhase.active) return;
      final sdp = _sdpOf(ev.data);
      final pc = s.pc;
      if (sdp == null || pc == null) return;
      // The caller restarts at most every [_restartEvery]: anything faster is not ours.
      final now = DateTime.now();
      final last = s.lastRestart;
      if (last != null && now.difference(last) < const Duration(seconds: 4)) return;
      s.lastRestart = now;
      // New candidates (with a fresh budget) wait until the new description is set.
      s.remoteDescriptionSet = false;
      s.remoteIce.clear();
      s.remoteIceCount = 0;
      try {
        await pc.setRemoteDescription(RTCSessionDescription(sdp, 'offer'));
        final answer = tuned(await pc.createAnswer());
        if (_session != s || s.ended) return;
        await pc.setLocalDescription(answer);
        if (_session != s || s.ended) return;
        s.remoteDescriptionSet = true;
        await _signal('answer', {'sdp': answer.sdp});
      } catch (e) {
        debugPrint('call: restart offer rejected: $e');
        if (_session == s) s.remoteDescriptionSet = true;
        return;
      }
      await _applyRemoteIce();
      _flushLocalIce();
    case 'ice':
      if (current == null || s == null) return;
      // Before the answer, only candidates from the device we will talk to matter.
      if (s.peerDevice != null && ev.deviceId != s.peerDevice) return;
      s.remoteIce.addAll(_candidatesOf(s, ev.data));
      await _applyRemoteIce();
    case 'media':
      if (current == null || s == null || ev.deviceId != s.peerDevice) return;
      final camera = ev.data?['camera'];
      if (camera is bool) _set((c) => c.copyWith(remoteCameraOn: camera));
    case 'busy':
      if (current != null && current.outgoing && current.phase == CallPhase.outgoing) {
        showToast(tr('call.busy', args: {'name': current.peerName}));
        await _endCall('busy', notifyPeer: false);
      }
    case 'hangup':
      if (current == null || s == null) {
        // The caller gave up before its offer reached us: the system call screen (shown by
        // the push) must not keep ringing, or open a call with no one on the other side.
        _endOrphanSystemCall(ev.callId);
        return;
      }
      // Once connected, only the device we talk to can end the call.
      if (s.peerDevice != null && ev.deviceId != s.peerDevice && current.phase != CallPhase.incoming) return;
      final reason = ev.data?['reason'] is String ? ev.data!['reason'] as String : '';
      if (current.outgoing && reason == 'calls_disabled') {
        // The peer turned calls off (their core answers for them).
        showToast(tr('call.callsDisabledBy', args: {'name': current.peerName}));
        await _endCall('unavailable', notifyPeer: false);
        return;
      }
      final status = current.outgoing
          ? (reason == 'declined' ? 'declined' : 'missed')
          : current.phase == CallPhase.incoming
              ? 'missed'
              : 'answered';
      await _endCall(status, notifyPeer: false);
  }
}

// --- pushes, notification actions and CallKit (state/push_handling.dart) ---------------------

/// Answered before the offer was fetched (notification action, CallKit, cold start).
String? _pendingAnswer;

/// Calls on the system call screen (CallKit) still waiting for their offer.
final _systemCallWatch = <String, Timer>{};

/// Ends [callId] on the system call screen if no call with this id is set up within [after]:
/// the caller hung up before its offer reached us, or the offer was too old and dropped by the
/// core. Answered, it would otherwise show a running timer with no one on the other side.
void watchSystemCall(String callId, Duration after) {
  if (!CallKit.known.contains(callId)) return;
  _systemCallWatch.remove(callId)?.cancel();
  _systemCallWatch[callId] = Timer(after, () {
    _systemCallWatch.remove(callId);
    if (callState.value?.callId == callId) return;
    _endOrphanSystemCall(callId);
  });
}

/// A call we never set up (no offer) ended: the system call screen and a pending answer go.
void _endOrphanSystemCall(String callId) {
  _systemCallWatch.remove(callId)?.cancel();
  final answered = _pendingAnswer == callId;
  if (answered) _pendingAnswer = null;
  unawaited(CallKit.end(callId, failed: answered));
}

/// Declined before the offer was fetched.
final _pendingDecline = <String>{};

/// Calls of another account on this phone shown by CallKit: answering opens the accounts list.
final _otherAccountCalls = <String>{};

/// Call pushes waiting for their offer (fallback notification if it does not come).
final _expectedCalls = <String, Timer>{};

void markOtherAccountCall(String callId) => _otherAccountCalls.add(callId);

/// "answer", "decline", or "" (notification tapped) for the call [callId], from an
/// incoming-call notification or the system call screen.
Future<void> handleCallAction(String action, String callId) async {
  if (callId.isEmpty) return;
  final call = callState.value;
  final current = call != null && call.callId == callId ? call : null;
  if (_otherAccountCalls.remove(callId)) {
    unawaited(CallKit.end(callId));
    // The call is for another account: show the accounts on this phone.
    if (action == 'answer') Notifications.instance.onOpenProfiles?.call();
    return;
  }
  switch (action) {
    case 'answer':
      unawaited(setShowOverLockScreen(true));
      if (current?.phase == CallPhase.incoming) {
        await acceptCall();
      } else if (current == null) {
        _pendingAnswer = callId;
        // The offer has a few seconds to arrive, or the call is over.
        watchSystemCall(callId, const Duration(seconds: 10));
        try {
          await backend.pushWake();
        } catch (_) {}
      }
    case 'decline':
      if (current != null) {
        await hangUp();
      } else {
        _pendingDecline.add(callId);
        if (_pendingAnswer == callId) _pendingAnswer = null;
      }
    default:
      // Notification tapped (or full-screen intent): the app shows the ringing call.
      if (current != null) unawaited(setShowOverLockScreen(true));
  }
}

/// A call push arrived for the open account: the offer should follow. If it does not within a
/// few seconds (fetch failed), a generic incoming-call notification still tells the user.
void expectCall(String callId) {
  if (callId.isEmpty || callState.value?.callId == callId || CallKit.known.contains(callId)) return;
  _expectedCalls.remove(callId)?.cancel();
  _expectedCalls[callId] = Timer(const Duration(seconds: 10), () {
    _expectedCalls.remove(callId);
    // Calls turned off: the core never delivers the offer and the caller is told.
    if (callState.value?.callId == callId || appInForeground || dndActive() || !callsAllowed() || !Platform.isAndroid) return;
    unawaited(Notifications.instance.showIncomingCall(
      callId: callId,
      title: kBrandName,
      body: tr('call.incomingTitle'),
      answerLabel: tr('call.accept'),
      declineLabel: tr('call.decline'),
    ));
  });
}

/// Random UUID v4 for the call id.
String _uuid() {
  final r = _rng;
  final b = List<int>.generate(16, (_) => r.nextInt(256));
  b[6] = (b[6] & 0x0f) | 0x40;
  b[8] = (b[8] & 0x3f) | 0x80;
  final h = b.map((x) => x.toRadixString(16).padLeft(2, '0')).join();
  return '${h.substring(0, 8)}-${h.substring(8, 12)}-${h.substring(12, 16)}-${h.substring(16, 20)}-${h.substring(20)}';
}

final _rng = Random.secure();

/// Android: keep gathering candidates during the call, so a network change (Wi-Fi <-> mobile
/// data) moves the call to the new network without waiting for an ICE restart.
const _networkChange = {'continualGatheringPolicy': 'gather_continually'};
