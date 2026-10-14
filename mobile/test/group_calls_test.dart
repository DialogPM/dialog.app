import 'package:chatapp/core/backend.dart';
import 'package:chatapp/state/group_calls.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('the smaller (account, device) makes the offer, exactly one side per pair', () {
    expect(isGroupOfferer('a', 2, 'b', 1), isTrue);
    expect(isGroupOfferer('b', 1, 'a', 2), isFalse);
    expect(isGroupOfferer('a', 1, 'a', 2), isTrue);
    const devs = [('x', 1), ('x', 2), ('y', 1), ('z', 9)];
    for (final a in devs) {
      for (final b in devs) {
        if (a == b) continue;
        expect(isGroupOfferer(a.$1, a.$2, b.$1, b.$2), isNot(isGroupOfferer(b.$1, b.$2, a.$1, a.$2)));
      }
    }
  });

  test('group call state parses', () {
    final info = GroupCallInfo.fromJson({
      'group_id': 'g',
      'conversation_id': 'c',
      'call_id': 'k',
      'started_by': 'a',
      'started_at': 5,
      'video': true,
      'participants': [
        {'account_id': 'a', 'device_id': 1, 'me': true, 'relay': false},
        {'account_id': 'b', 'device_id': 2, 'me': false, 'relay': true},
      ],
      'joined': true,
      'ringing': false,
      'ended': false,
      'max_participants': 8,
    });
    expect(info.participants.length, 2);
    expect(info.participants.last.key, 'b:2');
    expect(info.participants.last.relay, isTrue);
    expect(info.video, isTrue);
  });
}
