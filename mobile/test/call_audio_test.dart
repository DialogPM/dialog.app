import 'package:flutter_test/flutter_test.dart';
import 'package:chatapp/state/call_audio.dart';

const _chrome = 'v=0\r\n'
    'm=audio 9 UDP/TLS/RTP/SAVPF 111 63 9 0\r\n'
    'a=rtpmap:111 opus/48000/2\r\n'
    'a=fmtp:111 minptime=10;useinbandfec=0\r\n'
    'a=rtpmap:63 red/48000/2\r\n'
    'a=fmtp:63 111/111\r\n'
    'm=video 9 UDP/TLS/RTP/SAVPF 96 97\r\n'
    'a=rtpmap:97 red/90000\r\n';

void main() {
  test('prefers RED and turns on Opus FEC in audio only (same as the desktop app)', () {
    final out = tuneAudioSdp(_chrome);
    expect(out, contains('m=audio 9 UDP/TLS/RTP/SAVPF 63 111 9 0\r\n'));
    expect(out, contains('a=fmtp:111 minptime=10;useinbandfec=1\r\n'));
    expect(out, contains('m=video 9 UDP/TLS/RTP/SAVPF 96 97\r\n'));
    expect(tuneAudioSdp(out), out);
  });

  test('adds missing lines, leaves SDPs without Opus alone', () {
    final out = tuneAudioSdp('v=0\nm=audio 9 RTP/SAVPF 111 63\na=rtpmap:111 opus/48000/2\na=rtpmap:63 red/48000/2\n');
    expect(out, contains('m=audio 9 RTP/SAVPF 63 111\n'));
    expect(out, contains('a=rtpmap:111 opus/48000/2\na=fmtp:111 minptime=10;useinbandfec=1\n'));
    expect(out, contains('a=rtpmap:63 red/48000/2\na=fmtp:63 111/111\n'));
    const noOpus = 'v=0\nm=audio 9 RTP/SAVPF 0\na=rtpmap:0 PCMU/8000\n';
    expect(tuneAudioSdp(noOpus), noOpus);
  });

  test('grades loss and round trip', () {
    expect(classifyQuality(0.01, 0.1), QualityLevel.good);
    expect(classifyQuality(0.05, 0.1), QualityLevel.poor);
    expect(classifyQuality(0.2, null), QualityLevel.bad);
    expect(classifyQuality(0, 0.5), QualityLevel.poor);
  });
}
