import 'package:chatapp/state/call_checks.dart';
import 'package:flutter_test/flutter_test.dart';

// Same rules and samples as crates/chat-core/src/engine/call_checks.rs: the UIs and the core
// must agree on which remote descriptions reach WebRTC.
const _fp = 'sha-256 4A:AD:B9:B1:3F:82:18:3B:54:02:12:DF:3E:5D:49:6B:19:E5:7C:AB:3D:B4:0A:3E:4E:A9:66:9F:12:E5:8A:01';
const _fp2 = 'sha-256 4A:AD:B9:B1:3F:82:18:3B:54:02:12:DF:3E:5D:49:6B:19:E5:7C:AB:3D:B4:0A:3E:4E:A9:66:9F:12:E5:8A:02';

String _section(String kind, String mid, String setup, {String? fp = _fp}) => [
      'm=$kind 9 UDP/TLS/RTP/SAVPF 111 63',
      'c=IN IP4 0.0.0.0',
      'a=ice-ufrag:Kq3N',
      'a=ice-pwd:1kJ1iCZ0Wmm0bVjgnvGgZXdr',
      'a=ice-options:trickle',
      if (fp != null) 'a=fingerprint:$fp',
      'a=setup:$setup',
      'a=mid:$mid',
      'a=sendrecv',
      'a=rtcp-mux',
      'a=rtpmap:111 opus/48000/2',
    ].join('\r\n');

const _head = 'v=0\r\no=- 4611731400430051336 2 IN IP4 127.0.0.1\r\ns=-\r\nt=0 0\r\na=group:BUNDLE 0 1\r\n';

String _chromeVideoOffer() => '$_head${_section('audio', '0', 'actpass')}\r\n${_section('video', '1', 'actpass')}\r\n';
String _webkitAudioAnswer() => '$_head${_section('audio', '0', 'active')}\r\n';

void main() {
  test('accepts real descriptions', () {
    expect(validSdp(_chromeVideoOffer()), isTrue);
    expect(validSdp(_webkitAudioAnswer()), isTrue);
    expect(validSdp(_chromeVideoOffer().replaceAll('\r\n', '\n')), isTrue);
    // Fingerprint at session level only (Firefox style).
    expect(validSdp('v=0\r\na=fingerprint:$_fp\r\n${_section('audio', '0', 'actpass', fp: null)}\r\n'), isTrue);
  });

  test('rejects data channels, extra media, other transports', () {
    expect(validSdp('${_chromeVideoOffer()}m=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\na=sctp-port:5000\r\n'), isFalse);
    expect(validSdp('${_webkitAudioAnswer()}${_section('audio', '1', 'active')}\r\n'), isFalse);
    expect(validSdp(_webkitAudioAnswer().replaceAll('UDP/TLS/RTP/SAVPF', 'RTP/AVP')), isFalse);
    expect(validSdp(_webkitAudioAnswer().replaceAll('a=mid:0', 'a=mid:0\r\na=sctp-port:5000')), isFalse);
  });

  test('rejects fingerprint and setup problems', () {
    expect(validSdp('$_head${_section('audio', '0', 'actpass')}\r\n${_section('video', '1', 'actpass', fp: _fp2)}\r\n'), isFalse);
    expect(validSdp('$_head${_section('audio', '0', 'actpass', fp: null)}\r\n'), isFalse);
    expect(validSdp(_webkitAudioAnswer().replaceAll('sha-256', 'sha-1')), isFalse);
    expect(validSdp(_webkitAudioAnswer().replaceAll('a=mid:0', 'a=mid:0\r\na=fingerprint:$_fp')), isFalse);
    expect(validSdp(_webkitAudioAnswer().replaceAll('a=setup:active\r\n', '')), isFalse);
  });

  test('rejects malformed or oversized', () {
    expect(validSdp(''), isFalse);
    expect(validSdp('garbage'), isFalse);
    expect(validSdp(_webkitAudioAnswer().replaceAll('a=ice-options:trickle', 'a=ice-options:bogus')), isFalse);
    expect(validSdp(_webkitAudioAnswer().replaceAll('a=mid:0', 'a=mid:0\r\n${'a=x-pad:0\r\n' * 2000}')), isFalse);
    expect(validSdp(_webkitAudioAnswer().replaceAll('a=mid:0', 'a=mid:0\r\nbad line')), isFalse);
  });

  test('cleans ICE candidates', () {
    final good = {
      'candidate': 'candidate:842163049 1 udp 1677729535 203.0.113.7 51234 typ srflx raddr 0.0.0.0 rport 0',
      'sdpMid': '0',
      'sdpMLineIndex': 0,
    };
    expect(cleanCandidate(good)?.sdpMid, '0');
    expect(cleanCandidate({'candidate': 'a=candidate:1 1 udp 1 1.2.3.4 1 typ host'}), isNull);
    expect(cleanCandidate({'candidate': 'candidate:${'1' * 600}'}), isNull);
    expect(cleanCandidate({'candidate': 'candidate:1', 'sdpMid': 'x' * 33}), isNull);
    expect(cleanCandidate({'candidate': 'candidate:1', 'sdpMLineIndex': 7}), isNull);
    expect(cleanCandidate({'candidate': 'candidate:1\r\na=evil'}), isNull);
    expect(cleanCandidate('candidate:1'), isNull);
  });
}
