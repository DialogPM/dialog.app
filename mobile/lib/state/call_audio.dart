// Call quality on bad networks (port of app/src/lib/callAudio.ts, 1:1 and group calls):
// - tuneAudioSdp: Opus in-band FEC on, and RED (RFC 2198, every packet also carries the previous
//   one) preferred when both sides support it, so lost packets do not cut the voice;
// - prioritizeAudio: the voice gets the bandwidth first, video degrades before it;
// - QualityMeter: packet loss / round trip from getStats, for the "weak connection" notice and
//   the automatic camera stop of video calls.

import 'package:flutter/foundation.dart';
import 'package:flutter_webrtc/flutter_webrtc.dart';

/// Rewrites the audio sections of a local SDP (offer or answer) before it is applied.
String tuneAudioSdp(String sdp) {
  final eol = sdp.contains('\r\n') ? '\r\n' : '\n';
  final out = <String>[];
  var section = <String>[];
  void flush() {
    out.addAll(section.isNotEmpty && section.first.startsWith('m=audio') ? _tuneAudioSection(section) : section);
    section = <String>[];
  }

  for (final line in sdp.split(eol)) {
    if (line.startsWith('m=')) flush();
    section.add(line);
  }
  flush();
  return out.join(eol);
}

String? _payloadType(List<String> section, RegExp codec) {
  final rtpmap = RegExp(r'^a=rtpmap:(\d+) ([^/]+)/');
  for (final l in section) {
    final m = rtpmap.firstMatch(l);
    if (m != null && codec.hasMatch(m.group(2)!)) return m.group(1);
  }
  return null;
}

List<String> _tuneAudioSection(List<String> section) {
  final opus = _payloadType(section, RegExp(r'^opus$', caseSensitive: false));
  if (opus == null) return section;
  final lines = [...section];
  // Opus in-band forward error correction.
  final fmtpPrefix = 'a=fmtp:$opus ';
  final fmtp = lines.indexWhere((l) => l.startsWith(fmtpPrefix));
  if (fmtp >= 0) {
    final params = lines[fmtp]
        .substring(fmtpPrefix.length)
        .split(';')
        .where((p) => p.isNotEmpty && !p.trim().toLowerCase().startsWith('useinbandfec='))
        .toList();
    lines[fmtp] = '$fmtpPrefix${[...params, 'useinbandfec=1'].join(';')}';
  } else {
    final rtpmap = lines.indexWhere((l) => l.startsWith('a=rtpmap:$opus '));
    lines.insert(rtpmap + 1, '${fmtpPrefix}minptime=10;useinbandfec=1');
  }
  // RED first in the codec list: the other side then sends redundant audio to us.
  final red = _payloadType(lines, RegExp(r'^red$', caseSensitive: false));
  if (red != null) {
    final parts = lines.first.split(' ');
    final pts = parts.skip(3).where((p) => p != red);
    lines[0] = [...parts.take(3), red, ...pts].join(' ');
    if (!lines.any((l) => l.startsWith('a=fmtp:$red '))) {
      final at = lines.indexWhere((l) => l.startsWith('a=rtpmap:$red '));
      lines.insert(at + 1, 'a=fmtp:$red $opus/$opus');
    }
  }
  return lines;
}

/// A local description with tuned audio (use it for setLocalDescription and the signal).
RTCSessionDescription tuned(RTCSessionDescription d) => RTCSessionDescription(d.sdp == null ? null : tuneAudioSdp(d.sdp!), d.type);

/// Voice first: audio senders get high priority, video low (it degrades first).
Future<void> prioritizeAudio(RTCPeerConnection pc) async {
  try {
    for (final sender in await pc.getSenders()) {
      final kind = sender.track?.kind;
      if (kind == null) continue;
      final p = sender.parameters;
      final encodings = p.encodings;
      if (encodings == null || encodings.isEmpty) continue;
      final priority = kind == 'audio' ? RTCPriorityType.high : RTCPriorityType.low;
      for (final e in encodings) {
        e.priority = priority;
        e.networkPriority = priority;
      }
      await sender.setParameters(p);
    }
  } catch (e) {
    debugPrint('call priority: $e');
  }
}

enum QualityLevel { good, poor, bad }

/// Whose connection looks weak: ours (what we send is lost), the peer's, or unknown.
enum QualityWho { me, peer }

@immutable
class Quality {
  const Quality(this.level, this.who, this.outBitrate);
  final QualityLevel level;
  final QualityWho? who;

  /// Estimated bandwidth we can send with (bits/s), when known.
  final double? outBitrate;
}

QualityLevel classifyQuality(double loss, double? rtt) {
  final r = rtt ?? 0;
  if (loss > 0.1 || r > 0.7) return QualityLevel.bad;
  if (loss > 0.03 || r > 0.4) return QualityLevel.poor;
  return QualityLevel.good;
}

/// Below this sending bandwidth a video call stops our camera, so the voice stays clear.
const videoMinBitrate = 120000.0;

double? _num(Object? v) => v is num ? v.toDouble() : (v is String ? double.tryParse(v) : null);

/// Turns successive getStats() samples into a smoothed quality reading.
class QualityMeter {
  double _lost = 0;
  double _received = 0;
  QualityLevel _pending = QualityLevel.good;
  int _pendingCount = 0;
  QualityLevel _level = QualityLevel.good;

  Future<Quality> sample(RTCPeerConnection pc) async {
    final stats = await pc.getStats();
    var lost = 0.0;
    var received = 0.0;
    var outLoss = 0.0;
    double? rtt;
    double? outBitrate;
    String? pairId;
    for (final r in stats) {
      if (r.type == 'transport' && r.values['selectedCandidatePairId'] is String) pairId = r.values['selectedCandidatePairId'] as String;
    }
    for (final r in stats) {
      final v = r.values;
      final kind = v['kind'] ?? v['mediaType'];
      if (r.type == 'inbound-rtp' && kind == 'audio') {
        lost += _num(v['packetsLost']) ?? 0;
        received += _num(v['packetsReceived']) ?? 0;
      } else if (r.type == 'remote-inbound-rtp' && kind == 'audio') {
        final f = _num(v['fractionLost']) ?? 0;
        if (f > outLoss) outLoss = f;
        rtt = _num(v['roundTripTime']) ?? rtt;
      } else if (r.type == 'candidate-pair' && (r.id == pairId || (pairId == null && v['nominated'] == true && v['state'] == 'succeeded'))) {
        rtt = _num(v['currentRoundTripTime']) ?? rtt;
        outBitrate = _num(v['availableOutgoingBitrate']) ?? outBitrate;
      }
    }
    final dLost = (lost - _lost).clamp(0, double.infinity);
    final dRecv = (received - _received).clamp(0, double.infinity);
    _lost = lost;
    _received = received;
    final inLoss = dLost + dRecv > 0 ? dLost / (dLost + dRecv) : 0.0;
    final raw = classifyQuality(inLoss > outLoss ? inLoss.toDouble() : outLoss, rtt);
    // Two samples in a row before the shown level changes (no flicker).
    if (raw == _pending) {
      _pendingCount++;
    } else {
      _pending = raw;
      _pendingCount = 1;
    }
    if (_pendingCount >= 2) _level = raw;
    QualityWho? who;
    if (outLoss > 0.03 && outLoss >= inLoss * 1.5) {
      who = QualityWho.me;
    } else if (inLoss > 0.03) {
      who = QualityWho.peer;
    }
    return Quality(_level, who, outBitrate);
  }
}
