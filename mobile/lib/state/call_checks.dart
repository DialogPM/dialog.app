// Checks on remote call signals before they reach WebRTC. Mirrors
// crates/chat-core/src/engine/call_checks.rs (the core already drops what fails these; this is
// the second line of defense): at most one audio and one video section over UDP/TLS/RTP/SAVPF,
// no data channels, a single SHA-256 DTLS fingerprint, a=setup present, <=16 KB; ICE candidates
// capped and reduced to the fields WebRTC needs.

import 'dart:convert';

const maxSdpBytes = 16 * 1024;
const _maxSdpLine = 2048;
const _maxSdpCandidates = 50;

/// Remote ICE candidates accepted per call.
const maxRemoteIce = 50;
const _maxCandidateBytes = 512;
const _maxSdpMid = 32;
const _maxUfrag = 256;
const _maxMLines = 2;
const _proto = 'UDP/TLS/RTP/SAVPF';
const _iceOptions = {'trickle', 'renomination', 'ice2'};
const _setup = {'actpass', 'active', 'passive'};
final _lineStart = RegExp(r'^[a-z]=');
final _hexByte = RegExp(r'^[0-9a-fA-F]{2}$');
final _digits = RegExp(r'^\d+$');

class _Section {
  final fingerprints = <String>[];
  bool setup = false;
}

String? _sha256Fingerprint(String v) {
  final i = v.indexOf(' ');
  if (i < 0 || v.substring(0, i).toLowerCase() != 'sha-256') return null;
  final value = v.substring(i + 1).trim();
  final bytes = value.split(':');
  if (bytes.length != 32 || !bytes.every(_hexByte.hasMatch)) return null;
  return value.toUpperCase();
}

/// Whether a remote session description is one our clients would produce.
bool validSdp(String sdp) {
  if (sdp.isEmpty || utf8.encode(sdp).length > maxSdpBytes) return false;
  final session = _Section();
  final media = <_Section>[];
  var audio = 0, video = 0, candidates = 0;
  var first = true;
  for (final raw in sdp.split('\n')) {
    final line = raw.endsWith('\r') ? raw.substring(0, raw.length - 1) : raw;
    if (line.isEmpty) continue;
    if (line.length > _maxSdpLine || !_lineStart.hasMatch(line) || line.contains('\r')) return false;
    if (first) {
      if (line != 'v=0') return false;
      first = false;
      continue;
    }
    final type = line[0];
    final value = line.substring(2);
    if (type == 'm') {
      final parts = value.split(' ');
      if (parts.length < 4 || !_digits.hasMatch(parts[1]) || int.parse(parts[1]) > 65535 || parts[2] != _proto) {
        return false;
      }
      switch (parts[0]) {
        case 'audio':
          audio++;
        case 'video':
          video++;
        default:
          return false;
      }
      if (audio > 1 || video > 1) return false;
      media.add(_Section());
      continue;
    }
    if (type != 'a') continue;
    final colon = value.indexOf(':');
    final attr = colon < 0 ? value : value.substring(0, colon);
    final arg = colon < 0 ? '' : value.substring(colon + 1);
    final section = media.isEmpty ? session : media.last;
    switch (attr) {
      case 'fingerprint':
        final fp = _sha256Fingerprint(arg);
        if (fp == null) return false;
        section.fingerprints.add(fp);
      case 'setup':
        if (!_setup.contains(arg)) return false;
        section.setup = true;
      case 'ice-options':
        if (!arg.split(' ').every((t) => t.isEmpty || _iceOptions.contains(t))) return false;
      case 'candidate':
        if (++candidates > _maxSdpCandidates) return false;
      case 'sctp-port':
      case 'sctpmap':
        return false;
    }
  }
  if (first || media.isEmpty || media.length > _maxMLines || session.fingerprints.length > 1) return false;
  String? fingerprint = session.fingerprints.isEmpty ? null : session.fingerprints.first;
  for (final m in media) {
    if (m.fingerprints.length > 1 || (!m.setup && !session.setup)) return false;
    final fp = m.fingerprints.isEmpty ? null : m.fingerprints.first;
    if (fp == null && fingerprint == null) return false;
    if (fp != null) {
      if (fingerprint != null && fp != fingerprint) return false;
      fingerprint = fp;
    }
  }
  return true;
}

/// A remote ICE candidate (RTCIceCandidateInit fields) after checks.
class CleanCandidate {
  const CleanCandidate(this.candidate, this.sdpMid, this.sdpMLineIndex);
  final String candidate;
  final String? sdpMid;
  final int? sdpMLineIndex;
}

/// One remote ICE candidate reduced to the fields WebRTC needs, or null if malformed.
CleanCandidate? cleanCandidate(Object? c) {
  if (c is! Map) return null;
  final candidate = c['candidate'];
  if (candidate is! String ||
      !candidate.startsWith('candidate:') ||
      utf8.encode(candidate).length >= _maxCandidateBytes ||
      candidate.contains('\r') ||
      candidate.contains('\n')) {
    return null;
  }
  final mid = c['sdpMid'];
  if (mid != null && (mid is! String || mid.length > _maxSdpMid || mid.contains('\r') || mid.contains('\n'))) {
    return null;
  }
  final index = c['sdpMLineIndex'];
  if (index != null && (index is! int || index < 0 || index >= _maxMLines)) return null;
  final ufrag = c['usernameFragment'];
  if (ufrag != null && (ufrag is! String || ufrag.length > _maxUfrag)) return null;
  return CleanCandidate(candidate, mid as String?, index as int?);
}
