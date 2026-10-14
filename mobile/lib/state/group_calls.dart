// Group voice and video calls (port of app/src/store/groupCalls.ts): full mesh, one WebRTC peer
// connection per pair of participants, no media server. Signaling and the participant list
// come from the core (crates/chat-core/src/engine/group_calls.rs). Interoperates with the
// desktop app.
//
// The device with the smaller (account_id, device_id) makes the offer of a pair, so late
// joiners and people already in the call never offer to each other at the same time.

import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter_webrtc/flutter_webrtc.dart';

import '../app/globals.dart';
import '../core/backend.dart';
import '../core/models.dart';
import '../l10n/i18n.dart';
import '../services/call_sounds.dart';
import '../services/callkit.dart';
import '../services/notifications.dart';
import '../services/voice_fx.dart';
import '../widgets/toast.dart';
import 'call_audio.dart';
import 'call_checks.dart';
import 'call_settings.dart';
import 'calls.dart' show callCandidateAllowed, callHasTurn, callIceServers, callLocalMedia, callRelayUnavailable, callState;
import 'events.dart' show appInForeground;

/// Above this many participants the UI suggests audio only (each pair carries its own video).
const videoSuggestedMax = 4;
const _iceBatch = Duration(milliseconds: 120);
const _heartbeatEvery = Duration(seconds: 10);
const _statsEvery = Duration(milliseconds: 700);
/// A pair that stays down this long is rebuilt from scratch (new peer connection). Before that,
/// the offerer restarts ICE on the same connection (an offer with `restart: true`).
const _disconnectGrace = Duration(seconds: 20);
/// "disconnected" often recovers by itself: the restart waits this long first.
const _restartDelay = Duration(seconds: 2);
/// At most one ICE restart of a pair this often.
const _restartEvery = Duration(seconds: 8);
const _maxPairRetries = 3;
const _speakingLevel = 0.04;

enum GroupCallPhase { incoming, joining, active }

@immutable
class GroupPeer {
  final String key;
  final String accountId;
  final int deviceId;
  final String name;
  final String? avatar;
  final MediaStream? stream;
  final bool cameraOn;
  final bool connected;
  final bool speaking;

  const GroupPeer({
    required this.key,
    required this.accountId,
    required this.deviceId,
    required this.name,
    this.avatar,
    this.stream,
    this.cameraOn = true,
    this.connected = false,
    this.speaking = false,
  });

  bool get showsVideo => cameraOn && (stream?.getVideoTracks().isNotEmpty ?? false);

  GroupPeer copyWith({MediaStream? stream, bool clearStream = false, bool? cameraOn, bool? connected, bool? speaking}) => GroupPeer(
        key: key,
        accountId: accountId,
        deviceId: deviceId,
        name: name,
        avatar: avatar,
        stream: clearStream ? null : (stream ?? this.stream),
        cameraOn: cameraOn ?? this.cameraOn,
        connected: connected ?? this.connected,
        speaking: speaking ?? this.speaking,
      );
}

@immutable
class GroupCallUi {
  final String conversationId;
  final String callId;
  final String title;
  final String? avatar;
  final bool video;
  final GroupCallPhase phase;
  final bool muted;
  final bool cameraOn;
  final bool speaker;
  final bool speaking;
  final MediaStream? localStream;
  final Map<String, GroupPeer> peers;
  final int? joinedAt;
  final int maxParticipants;
  /// Our live voice effect (services/voice_fx.dart), "none" at the start of every call.
  final String voiceEffect;
  /// AI noise reduction of our microphone (starts from the setting).
  final bool denoise;

  const GroupCallUi({
    required this.conversationId,
    required this.callId,
    required this.title,
    this.avatar,
    required this.video,
    required this.phase,
    this.muted = false,
    required this.cameraOn,
    this.speaker = true,
    this.speaking = false,
    this.localStream,
    this.peers = const {},
    this.joinedAt,
    this.maxParticipants = 8,
    this.voiceEffect = 'none',
    this.denoise = false,
  });

  bool get hasCamera => localStream?.getVideoTracks().isNotEmpty ?? false;

  GroupCallUi copyWith({
    String? callId,
    GroupCallPhase? phase,
    bool? muted,
    bool? cameraOn,
    bool? speaker,
    bool? speaking,
    MediaStream? localStream,
    Map<String, GroupPeer>? peers,
    int? joinedAt,
    String? voiceEffect,
    bool? denoise,
  }) =>
      GroupCallUi(
        conversationId: conversationId,
        callId: callId ?? this.callId,
        title: title,
        avatar: avatar,
        video: video,
        phase: phase ?? this.phase,
        muted: muted ?? this.muted,
        cameraOn: cameraOn ?? this.cameraOn,
        speaker: speaker ?? this.speaker,
        speaking: speaking ?? this.speaking,
        localStream: localStream ?? this.localStream,
        peers: peers ?? this.peers,
        joinedAt: joinedAt ?? this.joinedAt,
        maxParticipants: maxParticipants,
        voiceEffect: voiceEffect ?? this.voiceEffect,
        denoise: denoise ?? this.denoise,
      );
}

/// The group call of this device (ringing or joined), null when there is none.
final groupCallState = ValueNotifier<GroupCallUi?>(null);

/// Calls in progress by conversation (for the "Join" banner).
final groupCallsOngoing = ValueNotifier<Map<String, GroupCallInfo>>(const {});

void _set(GroupCallUi Function(GroupCallUi c) update) {
  final c = groupCallState.value;
  if (c != null) groupCallState.value = update(c);
}

void _setPeer(String key, GroupPeer Function(GroupPeer p) update) {
  _set((c) {
    final p = c.peers[key];
    if (p == null) return c;
    return c.copyWith(peers: {...c.peers, key: update(p)});
  });
}

void _setOngoing(GroupCallInfo info) {
  final next = Map<String, GroupCallInfo>.of(groupCallsOngoing.value);
  if (info.ended) {
    next.remove(info.conversationId);
  } else {
    next[info.conversationId] = info;
  }
  groupCallsOngoing.value = next;
}

/// Same rule as the core: the smaller (account, device) offers.
bool isGroupOfferer(String aAccount, int aDevice, String bAccount, int bDevice) {
  final c = aAccount.compareTo(bAccount);
  if (c != 0) return c < 0;
  return aDevice < bDevice;
}

// --- non-reactive session state -------------------------------------------------------------

class _Pair {
  _Pair(this.key, this.accountId, this.deviceId, this.pc, this.offerer, this.relay);
  final String key;
  final String accountId;
  final int deviceId;
  final RTCPeerConnection pc;
  final bool offerer;
  final bool relay;
  bool remoteDescriptionSet = false;
  final remoteIce = <RTCIceCandidate>[];
  int remoteIceCount = 0;
  final localIce = <Map<String, dynamic>>[];
  Timer? iceTimer;
  Timer? disconnectTimer;
  Timer? restartTimer;
  /// Offerer: an ICE restart offer is waiting for its answer.
  bool restartPending = false;
  DateTime? lastRestart;
  bool closed = false;
}

class _Session {
  _Session(this.conversationId, this.callId);
  final String conversationId;
  String callId;
  String? meAccount;
  int? meDevice;
  MediaStream? stream;
  List<Map<String, dynamic>>? servers;
  final pairs = <String, _Pair>{};
  final retries = <String, int>{};
  Timer? heartbeat;
  Timer? stats;
  void Function()? stopSound;
  bool ended = false;
}

_Session? _session;

String _keyOf(String account, int device) => '$account:$device';

/// Id of a group call for CallKit (iOS): one call per conversation on this device.
String groupCallKitId(String conversationId) => 'g:$conversationId';

/// Name of a group call on the system call screen: the group only with notification previews.
String _callKitName(String conversationId) =>
    prefs.notifPreview ? (appData.conversations[conversationId]?.title ?? kBrandName) : kBrandName;

({String name, String? avatar}) _peerName(String accountId, String conversationId) {
  final d = appData;
  if (accountId == d.appState?.account?.accountId) return (name: tr('common.you'), avatar: d.profile?.avatarPath);
  final contact = d.contacts[accountId];
  GroupMember? member;
  for (final m in d.groups[conversationId]?.members ?? const <GroupMember>[]) {
    if (m.accountId == accountId) member = m;
  }
  return (name: contact?.displayName ?? member?.displayName ?? '', avatar: contact?.avatarPath ?? member?.avatarPath);
}

Future<void> _signal(_Pair pair, String kind, [Object? data]) async {
  final s = _session;
  if (s == null) return;
  try {
    await backend.groupCallSignal(
      conversationId: s.conversationId,
      callId: s.callId,
      accountId: pair.accountId,
      deviceId: pair.deviceId,
      kind: kind,
      data: data,
    );
  } catch (e) {
    debugPrint('group call signal $kind failed: $e');
  }
}

void _flushLocalIce(_Pair pair) {
  if (pair.localIce.isEmpty || pair.closed) return;
  final candidates = List<Map<String, dynamic>>.of(pair.localIce);
  pair.localIce.clear();
  unawaited(_signal(pair, 'ice', {'candidates': candidates}));
}

void _closePair(_Pair pair) {
  pair.closed = true;
  pair.iceTimer?.cancel();
  pair.disconnectTimer?.cancel();
  pair.restartTimer?.cancel();
  if (_session?.pairs[pair.key] == pair) _session?.pairs.remove(pair.key);
  unawaited(pair.pc.close().catchError((_) {}));
}

Future<_Pair?> _newPair(String accountId, int deviceId, bool relay) async {
  final s = _session;
  final stream = s?.stream;
  final servers = s?.servers;
  if (s == null || stream == null || servers == null || s.meAccount == null) return null;
  final key = _keyOf(accountId, deviceId);
  // Pairs with people who are not our contacts go through the relay only (no IP address leak);
  // with "Calls only through the Dialog server" every pair does (_enter refuses the call when
  // there is no TURN server).
  var policy = 'all';
  if (relay || relayOnlyCalls()) {
    if (callHasTurn(servers)) {
      policy = 'relay';
    } else {
      debugPrint('group call: relay-only requested but no TURN server is configured');
    }
  }
  final pc = await createPeerConnection({'iceServers': servers, 'sdpSemantics': 'unified-plan', 'iceTransportPolicy': policy, ..._networkChange});
  final offerer = isGroupOfferer(s.meAccount!, s.meDevice!, accountId, deviceId);
  for (final track in stream.getTracks()) {
    await pc.addTrack(track, stream);
  }
  if (offerer && groupCallState.value?.video == true && stream.getVideoTracks().isEmpty) {
    await pc.addTransceiver(
      kind: RTCRtpMediaType.RTCRtpMediaTypeVideo,
      init: RTCRtpTransceiverInit(direction: TransceiverDirection.RecvOnly),
    );
  }
  final pair = _Pair(key, accountId, deviceId, pc, offerer, relay);
  if (_session != s) {
    await pc.close();
    return null;
  }
  pc.onIceCandidate = (c) {
    if (_session != s || pair.closed || !callCandidateAllowed(c.candidate, policy == 'relay')) return;
    pair.localIce.add(c.toMap() as Map<String, dynamic>);
    pair.iceTimer?.cancel();
    pair.iceTimer = Timer(_iceBatch, () => _flushLocalIce(pair));
  };
  pc.onTrack = (e) {
    if (_session != s || pair.closed || e.streams.isEmpty) return;
    final remote = e.streams.first;
    _setPeer(key, (p) => p.copyWith(stream: remote));
  };
  pc.onConnectionState = (state) {
    if (_session != s || pair.closed) return;
    switch (state) {
      case RTCPeerConnectionState.RTCPeerConnectionStateConnected:
        pair.disconnectTimer?.cancel();
        pair.disconnectTimer = null;
        pair.restartTimer?.cancel();
        pair.restartTimer = null;
        _setPeer(key, (p) => p.copyWith(connected: true));
        unawaited(Helper.setSpeakerphoneOn(groupCallState.value?.speaker ?? true).catchError((_) {}));
        unawaited(prioritizeAudio(pc));
      case RTCPeerConnectionState.RTCPeerConnectionStateDisconnected:
        _setPeer(key, (p) => p.copyWith(connected: false));
        pair.disconnectTimer ??= Timer(_disconnectGrace, () => _rebuildPair(pair));
        if (pair.offerer) {
          pair.restartTimer?.cancel();
          pair.restartTimer = Timer(_restartDelay, () => unawaited(_restartPair(pair)));
        }
      case RTCPeerConnectionState.RTCPeerConnectionStateFailed:
        _setPeer(key, (p) => p.copyWith(connected: false));
        // One ICE restart first (the offerer makes it, the other side waits for it); the pair
        // is rebuilt if it is still down after the grace period.
        pair.disconnectTimer ??= Timer(_disconnectGrace, () => _rebuildPair(pair));
        if (pair.offerer) {
          pair.restartTimer?.cancel();
          unawaited(_restartPair(pair));
        }
      default:
        break;
    }
  };
  s.pairs[key] = pair;
  final who = _peerName(accountId, s.conversationId);
  _set((c) => c.copyWith(peers: {
        ...c.peers,
        key: GroupPeer(
          key: key,
          accountId: accountId,
          deviceId: deviceId,
          name: who.name,
          avatar: who.avatar,
          cameraOn: c.peers[key]?.cameraOn ?? true,
        ),
      }));
  return pair;
}

/// ICE restart of a pair that went down, on the same peer connection (offerer side): an offer
/// marked `restart`, answered in place by the other side. Retried while the pair stays down.
Future<void> _restartPair(_Pair pair) async {
  pair.restartTimer = null;
  final s = _session;
  if (s == null || pair.closed || s.pairs[pair.key] != pair || !pair.offerer) return;
  if (!pair.remoteDescriptionSet && !pair.restartPending) return;
  if (pair.pc.connectionState == RTCPeerConnectionState.RTCPeerConnectionStateConnected) return;
  final now = DateTime.now();
  final last = pair.lastRestart;
  if (last != null && now.difference(last) < _restartEvery) {
    pair.restartTimer = Timer(_restartEvery - now.difference(last), () => unawaited(_restartPair(pair)));
    return;
  }
  pair.lastRestart = now;
  try {
    await pair.pc.restartIce();
    final offer = tuned(await pair.pc.createOffer({'iceRestart': true}));
    if (_session != s || pair.closed) return;
    await pair.pc.setLocalDescription(offer);
    if (_session != s || pair.closed) return;
    pair.restartPending = true;
    pair.remoteDescriptionSet = false;
    pair.remoteIce.clear();
    pair.remoteIceCount = 0;
    await _signal(pair, 'offer', {'sdp': offer.sdp, 'video': groupCallState.value?.video ?? false, 'restart': true});
  } catch (e) {
    debugPrint('group call: ICE restart failed: $e');
  }
  if (_session == s && !pair.closed && pair.disconnectTimer != null) {
    pair.restartTimer?.cancel();
    pair.restartTimer = Timer(_restartEvery, () => unawaited(_restartPair(pair)));
  }
}

/// A failed pair starts over: the offerer offers again (a few times at most); the other side
/// asks it to.
void _rebuildPair(_Pair pair) {
  final s = _session;
  if (s == null || s.pairs[pair.key] != pair) return;
  _closePair(pair);
  _setPeer(pair.key, (p) => p.copyWith(connected: false, clearStream: true));
  if (!pair.offerer) {
    unawaited(_signal(pair, 'hangup', {'reason': 'restart'}));
    return;
  }
  _reoffer(s, pair);
}

void _reoffer(_Session s, _Pair pair) {
  final retries = s.retries[pair.key] ?? 0;
  if (retries >= _maxPairRetries) return;
  s.retries[pair.key] = retries + 1;
  Timer(const Duration(milliseconds: 1500), () async {
    if (_session != s || s.pairs.containsKey(pair.key)) return;
    final next = await _newPair(pair.accountId, pair.deviceId, pair.relay);
    if (next != null) await _makeOffer(next);
  });
}

Future<void> _makeOffer(_Pair pair) async {
  final s = _session;
  final offer = tuned(await pair.pc.createOffer());
  await pair.pc.setLocalDescription(offer);
  if (_session != s || pair.closed) return;
  await _signal(pair, 'offer', {'sdp': offer.sdp, 'video': groupCallState.value?.video ?? false});
  final call = groupCallState.value;
  if (call != null && call.video && !call.cameraOn) unawaited(_signal(pair, 'media', {'camera': false}));
}

/// Answers an ICE restart offer on the existing peer connection; false if it could not be
/// applied (the caller then replaces the pair).
Future<bool> _answerRestart(_Session s, _Pair pair, String sdp) async {
  final now = DateTime.now();
  final last = pair.lastRestart;
  // The offerer restarts at most every [_restartEvery]: anything faster is not ours.
  if (last != null && now.difference(last) < const Duration(seconds: 4)) return true;
  pair.lastRestart = now;
  pair.remoteDescriptionSet = false;
  pair.remoteIce.clear();
  pair.remoteIceCount = 0;
  try {
    await pair.pc.setRemoteDescription(RTCSessionDescription(sdp, 'offer'));
    final answer = tuned(await pair.pc.createAnswer());
    if (_session != s || pair.closed) return true;
    await pair.pc.setLocalDescription(answer);
    if (_session != s || pair.closed) return true;
    pair.remoteDescriptionSet = true;
    await _signal(pair, 'answer', {'sdp': answer.sdp});
  } catch (e) {
    debugPrint('group call: restart offer rejected: $e');
    return false;
  }
  await _applyRemoteIce(pair);
  _flushLocalIce(pair);
  return true;
}

Future<void> _applyRemoteIce(_Pair pair) async {
  if (!pair.remoteDescriptionSet) return;
  final pending = List<RTCIceCandidate>.of(pair.remoteIce);
  pair.remoteIce.clear();
  for (final c in pending) {
    try {
      await pair.pc.addCandidate(c);
    } catch (e) {
      debugPrint('bad ICE candidate: $e');
    }
  }
}

/// Brings the peer connections in line with the participant list from the core.
Future<void> _syncParticipants(GroupCallInfo info) async {
  final s = _session;
  if (s == null || s.stream == null || info.callId != s.callId) return;
  for (final p in info.participants) {
    if (p.me) {
      s.meAccount = p.accountId;
      s.meDevice = p.deviceId;
    }
  }
  if (s.meAccount == null) return;
  final present = <String>{};
  for (final p in info.participants) {
    if (p.me) continue;
    present.add(p.key);
  }
  for (final pair in s.pairs.values.toList()) {
    if (!present.contains(pair.key)) _closePair(pair);
  }
  // Tiles for everyone in the call, also before their connection is up.
  _set((c) {
    final peers = <String, GroupPeer>{
      for (final e in c.peers.entries)
        if (present.contains(e.key)) e.key: e.value,
    };
    for (final p in info.participants) {
      if (p.me || peers.containsKey(p.key)) continue;
      final who = _peerName(p.accountId, info.conversationId);
      peers[p.key] = GroupPeer(key: p.key, accountId: p.accountId, deviceId: p.deviceId, name: who.name, avatar: who.avatar, cameraOn: info.video);
    }
    return c.copyWith(peers: peers);
  });
  for (final p in info.participants) {
    if (p.me || s.pairs.containsKey(p.key)) continue;
    // Only the offerer opens the pair; the other side waits for its offer.
    if (isGroupOfferer(s.meAccount!, s.meDevice!, p.accountId, p.deviceId)) {
      final pair = await _newPair(p.accountId, p.deviceId, p.relay);
      if (pair != null) await _makeOffer(pair);
    }
  }
}

void _stopSound() {
  _session?.stopSound?.call();
  _session?.stopSound = null;
}

/// [local]: left / declined by the user here (for CallKit).
void _cleanup({bool local = false}) {
  final s = _session;
  if (s == null) return;
  s.ended = true;
  unawaited(CallKit.end(groupCallKitId(s.conversationId), local: local));
  _stopSound();
  s.heartbeat?.cancel();
  s.stats?.cancel();
  unawaited(VoiceFxNative.configure('none', false));
  final pairs = s.pairs.values.toList();
  for (final p in pairs) {
    _closePair(p);
  }
  final stream = s.stream;
  _session = null;
  groupCallState.value = null;
  // Released after the call screen has dropped its renderers (next frames).
  Future<void>.delayed(const Duration(milliseconds: 400), () async {
    for (final t in stream?.getTracks() ?? const <MediaStreamTrack>[]) {
      await t.stop().catchError((_) {});
    }
    await stream?.dispose().catchError((_) {});
  });
}

GroupCallUi _uiFor(GroupCallInfo info, GroupCallPhase phase) {
  final conv = appData.conversations[info.conversationId];
  return GroupCallUi(
    conversationId: info.conversationId,
    callId: info.callId,
    title: conv?.title ?? '',
    avatar: conv?.avatarPath,
    video: info.video,
    phase: phase,
    cameraOn: info.video,
    speaker: true,
    maxParticipants: info.maxParticipants,
  );
}

double? _level(Map<dynamic, dynamic> values) {
  final v = values['audioLevel'];
  if (v is num) return v.toDouble();
  if (v is String) return double.tryParse(v);
  return null;
}

/// Audio levels from the stats: who is speaking right now.
Future<void> _pollSpeaking(_Session s) async {
  if (_session != s) return;
  final speaking = <String, bool>{};
  var me = false;
  for (final pair in s.pairs.values.toList()) {
    try {
      final reports = await pair.pc.getStats();
      for (final r in reports) {
        final level = _level(r.values);
        if (level == null || r.values['kind'] != 'audio') continue;
        if (r.type == 'inbound-rtp') speaking[pair.key] = level > _speakingLevel;
        if (r.type == 'media-source' && level > _speakingLevel) me = true;
      }
    } catch (_) {}
  }
  if (_session != s) return;
  _set((c) {
    final peers = {for (final e in c.peers.entries) e.key: e.value.copyWith(speaking: speaking[e.key] ?? false)};
    return c.copyWith(peers: peers, speaking: me && !c.muted);
  });
}

Future<void> _enter(_Session s, bool video, Future<GroupCallInfo> Function() join) async {
  final servers = await callIceServers();
  if (_session != s) return;
  if (callRelayUnavailable(servers)) {
    _cleanup();
    return;
  }
  s.servers = servers;
  final stream = await callLocalMedia(video);
  if (_session != s) {
    for (final t in stream?.getTracks() ?? const <MediaStreamTrack>[]) {
      await t.stop();
    }
    await stream?.dispose();
    return;
  }
  if (stream == null) {
    _cleanup();
    return;
  }
  s.stream = stream;
  _set((c) => c.copyWith(localStream: stream, cameraOn: stream.getVideoTracks().isNotEmpty));
  // A fresh effect processor for this call, with the noise reduction setting applied.
  await VoiceFxNative.newCall();
  final denoise = prefs.callDenoise && await VoiceFxNative.configure('none', true);
  if (!prefs.callDenoise) await VoiceFxNative.configure('none', false);
  _set((c) => c.copyWith(voiceEffect: 'none', denoise: denoise));
  unawaited(Helper.setSpeakerphoneOn(true).catchError((_) {}));
  GroupCallInfo info;
  try {
    info = await join();
  } catch (e) {
    debugPrint('group call join failed: $e');
    showToast(tr('groupCall.joinFailed'), error: true);
    _cleanup();
    return;
  }
  if (_session != s) {
    unawaited(backend.groupCallLeave(s.conversationId).catchError((_) {}));
    return;
  }
  s.callId = info.callId;
  _set((c) => c.copyWith(callId: info.callId, phase: GroupCallPhase.active, joinedAt: DateTime.now().millisecondsSinceEpoch));
  unawaited(CallKit.connected(groupCallKitId(s.conversationId)));
  _setOngoing(info);
  s.heartbeat = Timer.periodic(_heartbeatEvery, (_) => unawaited(backend.groupCallHeartbeat(s.conversationId).catchError((_) {})));
  s.stats = Timer.periodic(_statsEvery, (_) => unawaited(_pollSpeaking(s)));
  await _syncParticipants(info);
}

bool _canCall() => callState.value == null && groupCallState.value == null;

// --- public actions -------------------------------------------------------------------------

/// Starts a call in a group conversation (or joins the one in progress).
Future<void> startGroupCall(String conversationId, {bool video = false}) async {
  final conv = appData.conversations[conversationId];
  if (conv == null || !conv.isGroup || !_canCall() || blockedByCallSettings()) return;
  final ongoing = groupCallsOngoing.value[conversationId];
  if (ongoing != null && !ongoing.ended) return joinGroupCall(conversationId, video: video);
  final s = _Session(conversationId, '');
  _session = s;
  groupCallState.value = GroupCallUi(
    conversationId: conversationId,
    callId: '',
    title: conv.title,
    avatar: conv.avatarPath,
    video: video,
    phase: GroupCallPhase.joining,
    cameraOn: video,
  );
  unawaited(CallKit.startCall(groupCallKitId(conversationId), name: _callKitName(conversationId), video: video));
  await _enter(s, video, () => backend.groupCallStart(conversationId, video: video));
}

/// Joins the call in progress (from the banner or the ring screen).
Future<void> joinGroupCall(String conversationId, {bool? video}) async {
  final current = groupCallState.value;
  final ringing = current?.phase == GroupCallPhase.incoming && current?.conversationId == conversationId;
  if (!ringing && !_canCall()) return;
  if (blockedByCallSettings()) {
    if (ringing) _cleanup();
    return;
  }
  final info = groupCallsOngoing.value[conversationId];
  if (info == null || info.ended) {
    if (ringing) _cleanup();
    return;
  }
  final v = video ?? info.video;
  if (ringing) {
    _stopSound();
    unawaited(CallKit.answered(groupCallKitId(conversationId)));
  } else {
    _session = _Session(conversationId, info.callId);
    unawaited(CallKit.startCall(groupCallKitId(conversationId), name: _callKitName(conversationId), video: v));
  }
  final s = _session!;
  groupCallState.value = _uiFor(info, GroupCallPhase.joining);
  await _enter(s, v, () => backend.groupCallJoin(conversationId, info.callId, video: v));
}

Future<void> declineGroupCall() async {
  final call = groupCallState.value;
  if (call == null || call.phase != GroupCallPhase.incoming) return;
  _cleanup(local: true);
  try {
    await backend.groupCallDecline(call.conversationId);
  } catch (_) {}
}

Future<void> leaveGroupCall() async {
  final call = groupCallState.value;
  if (call == null) return;
  if (call.phase == GroupCallPhase.incoming) return declineGroupCall();
  final s = _session;
  if (s != null) {
    for (final pair in s.pairs.values.toList()) {
      unawaited(_signal(pair, 'hangup', {'reason': 'left'}));
    }
  }
  _cleanup(local: true);
  try {
    await backend.groupCallLeave(call.conversationId);
  } catch (e) {
    debugPrint('group call leave: $e');
  }
}

/// Switches our voice effect and/or the noise reduction for everyone in the call.
Future<void> setGroupVoiceEffect({String? effect, bool? denoise}) async {
  final call = groupCallState.value;
  if (call == null) return;
  final e = effect ?? call.voiceEffect;
  final d = denoise ?? call.denoise;
  if (await VoiceFxNative.configure(e, d)) {
    _set((c) => c.copyWith(voiceEffect: e, denoise: d));
  } else {
    showToast(tr('voiceFx.error'), error: true);
  }
}

void toggleGroupMute() {
  final call = groupCallState.value;
  if (call != null) setGroupMuted(!call.muted);
}

/// Mutes / unmutes the microphone (call screen, or the system call screen through CallKit).
void setGroupMuted(bool muted) {
  final call = groupCallState.value;
  final stream = _session?.stream;
  if (call == null || stream == null || call.muted == muted) return;
  for (final t in stream.getAudioTracks()) {
    t.enabled = !muted;
  }
  _set((c) => c.copyWith(muted: muted, speaking: false));
  unawaited(CallKit.setMuted(groupCallKitId(call.conversationId), muted));
}

/// The system call screen (CallKit) answered / ended the group call [callKitId].
Future<void> handleGroupCallKitAction(String action, String callKitId) async {
  final call = groupCallState.value;
  if (call == null || groupCallKitId(call.conversationId) != callKitId) return;
  if (action == 'answer') {
    if (call.phase == GroupCallPhase.incoming) await joinGroupCall(call.conversationId);
  } else {
    await leaveGroupCall();
  }
}

void toggleGroupCamera() {
  final call = groupCallState.value;
  final s = _session;
  final tracks = s?.stream?.getVideoTracks() ?? const <MediaStreamTrack>[];
  if (call == null || s == null || tracks.isEmpty) return;
  final on = !call.cameraOn;
  tracks.first.enabled = on;
  _set((c) => c.copyWith(cameraOn: on));
  for (final pair in s.pairs.values) {
    unawaited(_signal(pair, 'media', {'camera': on}));
  }
}

Future<void> switchGroupCamera() async {
  final tracks = _session?.stream?.getVideoTracks() ?? const <MediaStreamTrack>[];
  if (tracks.isEmpty) return;
  try {
    await Helper.switchCamera(tracks.first);
  } catch (e) {
    debugPrint('switch camera: $e');
  }
}

Future<void> toggleGroupSpeaker() async {
  final call = groupCallState.value;
  if (call == null) return;
  final on = !call.speaker;
  _set((c) => c.copyWith(speaker: on));
  try {
    await Helper.setSpeakerphoneOn(on);
  } catch (e) {
    debugPrint('speakerphone: $e');
  }
}

/// Loads the call in progress of a conversation (for the banner when it is opened).
Future<void> refreshGroupCall(String conversationId) async {
  try {
    final info = await backend.groupCallState(conversationId);
    final next = Map<String, GroupCallInfo>.of(groupCallsOngoing.value);
    if (info != null && !info.ended) {
      next[conversationId] = info;
    } else {
      next.remove(conversationId);
    }
    groupCallsOngoing.value = next;
  } catch (_) {}
}

// --- events from the core ---------------------------------------------------------------------

Future<void> handleGroupCall(GroupCallInfo info) async {
  _setOngoing(info);
  final call = groupCallState.value;
  final s = _session;
  if (call != null && call.conversationId == info.conversationId) {
    if (call.phase == GroupCallPhase.incoming) {
      // Answered or declined on another device, the ring timed out, or everyone left.
      if (info.ended || !info.ringing || info.callId != call.callId) _cleanup();
      return;
    }
    if (call.phase == GroupCallPhase.active && s != null && info.callId != s.callId && !info.ended) {
      // Two calls were started at once and ours lost: move over to the other one.
      final video = call.video;
      _cleanup();
      await joinGroupCall(info.conversationId, video: video);
      return;
    }
    if (call.phase == GroupCallPhase.active) await _syncParticipants(info);
    return;
  }
  if (!info.ringing || info.ended || call != null || callState.value != null) return;
  final ns = _Session(info.conversationId, info.callId);
  _session = ns;
  groupCallState.value = _uiFor(info, GroupCallPhase.incoming);
  // iOS rings through CallKit when it takes the call: no second ringtone or notification.
  if (await CallKit.reportIncoming(groupCallKitId(info.conversationId), name: _callKitName(info.conversationId), video: info.video)) {
    return;
  }
  if (_session != ns || ns.ended) return;
  final stop = await startRingtone();
  if (_session != ns || ns.ended) {
    stop();
  } else {
    ns.stopSound = stop;
  }
  if (!appInForeground) {
    final title = appData.conversations[info.conversationId]?.title ?? '';
    unawaited(prefs.notifPreview
        ? Notifications.instance.showSimple(tr('groupCall.incomingTitle'), title)
        : Notifications.instance.showSimple(kBrandName, tr('groupCall.incomingTitle')));
  }
}

String? _sdpOf(Json? data) {
  final sdp = data?['sdp'];
  if (sdp is! String || !validSdp(sdp)) return null;
  return sdp;
}

bool _relayFor(GroupCallSignalEvent ev) {
  if (ev.relay) return true;
  for (final p in groupCallsOngoing.value[ev.conversationId]?.participants ?? const <GroupCallParticipant>[]) {
    if (p.accountId == ev.accountId && p.deviceId == ev.deviceId) return p.relay;
  }
  return false;
}

Future<void> handleGroupCallSignal(GroupCallSignalEvent ev) async {
  final s = _session;
  final call = groupCallState.value;
  if (s == null || call == null || call.phase != GroupCallPhase.active || ev.callId != s.callId || s.meAccount == null) return;
  final key = _keyOf(ev.accountId, ev.deviceId);
  var pair = s.pairs[key];
  switch (ev.kind) {
    case 'offer':
      final sdp = _sdpOf(ev.data);
      if (sdp == null || isGroupOfferer(s.meAccount!, s.meDevice!, ev.accountId, ev.deviceId)) return;
      // ICE restart of a pair that is up (or was): answered on the same peer connection.
      if (pair != null && !pair.closed && pair.remoteDescriptionSet && ev.data?['restart'] == true) {
        if (await _answerRestart(s, pair, sdp)) return;
        if (pair.closed) return;
      }
      // A new offer replaces the pair (the other side rebuilt it).
      if (pair != null) _closePair(pair);
      pair = await _newPair(ev.accountId, ev.deviceId, _relayFor(ev));
      if (pair == null) return;
      await pair.pc.setRemoteDescription(RTCSessionDescription(sdp, 'offer'));
      pair.remoteDescriptionSet = true;
      final answer = tuned(await pair.pc.createAnswer());
      await pair.pc.setLocalDescription(answer);
      if (_session != s || pair.closed) return;
      await _signal(pair, 'answer', {'sdp': answer.sdp});
      if (call.video && groupCallState.value?.cameraOn == false) unawaited(_signal(pair, 'media', {'camera': false}));
      await _applyRemoteIce(pair);
      _flushLocalIce(pair);
    case 'answer':
      final sdp = _sdpOf(ev.data);
      if (sdp != null && pair != null && pair.offerer && pair.restartPending) {
        // Answer to our ICE restart. An answer from a new peer connection (an older app
        // rebuilt the pair instead) does not fit: the pair is rebuilt here too.
        pair.restartPending = false;
        try {
          await pair.pc.setRemoteDescription(RTCSessionDescription(sdp, 'answer'));
        } catch (e) {
          debugPrint('group call: restart answer rejected: $e');
          _rebuildPair(pair);
          return;
        }
        pair.remoteDescriptionSet = true;
        await _applyRemoteIce(pair);
        _flushLocalIce(pair);
        return;
      }
      if (sdp == null || pair == null || !pair.offerer || pair.remoteDescriptionSet) return;
      await pair.pc.setRemoteDescription(RTCSessionDescription(sdp, 'answer'));
      pair.remoteDescriptionSet = true;
      await _applyRemoteIce(pair);
      _flushLocalIce(pair);
    case 'ice':
      if (pair == null) return;
      final list = ev.data?['candidates'];
      if (list is! List) return;
      for (final raw in list) {
        if (pair.remoteIceCount >= maxRemoteIce) break;
        final c = cleanCandidate(raw);
        if (c == null) continue;
        pair.remoteIceCount++;
        pair.remoteIce.add(RTCIceCandidate(c.candidate, c.sdpMid, c.sdpMLineIndex));
      }
      await _applyRemoteIce(pair);
    case 'media':
      final camera = ev.data?['camera'];
      if (camera is bool) _setPeer(key, (p) => p.copyWith(cameraOn: camera));
    case 'hangup':
      if (pair == null) return;
      _closePair(pair);
      _setPeer(key, (p) => p.copyWith(connected: false, clearStream: true));
      // The other side lost the connection and is still in the call: offer again.
      if (pair.offerer && ev.data?['reason'] == 'restart') _reoffer(s, pair);
  }
}

/// Android: keep gathering candidates during the call, so a network change (Wi-Fi <-> mobile
/// data) moves the call to the new network without waiting for an ICE restart.
const _networkChange = {'continualGatheringPolicy': 'gather_continually'};
