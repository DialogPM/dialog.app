import 'package:chatapp/core/backend.dart';
import 'package:chatapp/core/models.dart';
import 'package:chatapp/state/calls.dart' show callCandidateAllowed;
import 'package:chatapp/util/format.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('call settings parse with safe defaults', () {
    expect(CallSettings.fromJson({'allow_calls': false, 'relay_only': true}), const CallSettings(allowCalls: false, relayOnly: true));
    expect(CallSettings.fromJson({}), CallSettings.defaults);
    expect(CallSettings.defaults.allowCalls, isTrue);
    expect(CallSettings.defaults.relayOnly, isFalse);
  });

  test('call to someone who does not accept calls', () {
    const ev = SystemEvent(type: 'call', actorName: 'You', targetNames: [], value: '{"status":"unavailable","duration_ms":0}');
    expect(systemText(ev, 'en'), 'Calls not accepted');
    expect(systemText(ev, 'ro'), 'Apeluri neacceptate');
  });

  test('only relay candidates leave the device when relayed', () {
    const host = 'candidate:1 1 udp 2122260223 192.168.1.5 54321 typ host generation 0';
    const relay = 'candidate:3 1 udp 41885439 203.0.113.7 3478 typ relay raddr 0.0.0.0 rport 0';
    expect(callCandidateAllowed(host, true), isFalse);
    expect(callCandidateAllowed(relay, true), isTrue);
    expect(callCandidateAllowed(host, false), isTrue);
    expect(callCandidateAllowed(null, false), isFalse);
  });
}
