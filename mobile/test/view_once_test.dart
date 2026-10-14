import 'dart:async';
import 'dart:convert';

import 'package:chatapp/core/backend.dart';
import 'package:chatapp/core/models.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('Message parses view_once', () {
    Message parse(Object? state) => Message.fromJson(jsonDecode(jsonEncode({
          'id': 'm1',
          'conversation_id': 'c1',
          'sender_account_id': 'a1',
          'sender_name': 'Ana',
          'outgoing': false,
          'kind': 'image',
          'attachments': [
            {'id': 'x', 'mime': 'image/jpeg', 'file_name': 'a.jpg', 'size': 10, 'download_state': 'pending'},
          ],
          'status': 'delivered',
          'sent_at': 1,
          'received_at': 2,
          'deleted': false,
          'forwarded': false,
          'view_once': state,
        })) as Map<String, dynamic>);
    expect(parse('unopened').viewOnce, 'unopened');
    expect(parse('unopened').isViewOnce, isTrue);
    expect(parse('opened').viewOnce, 'opened');
    expect(parse(null).isViewOnce, isFalse);
    expect(parse(null).attachments.single.localPath, isNull);
  });

  test('MessagePreview parses view_once', () {
    final p = MessagePreview.fromJson({'id': 'm1', 'kind': 'video', 'outgoing': true, 'sent_at': 1, 'deleted': false, 'view_once': true});
    expect(p.viewOnce, isTrue);
    expect(MessagePreview.fromJson({'id': 'm2', 'kind': 'video', 'sent_at': 1}).viewOnce, isFalse);
  });

  test('backend view-once commands', () async {
    final calls = <(String, Object?)>[];
    final backend = Backend((cmd, req) async {
      calls.add((cmd, jsonDecode(req)));
      return switch (cmd) {
        'open_view_once' => jsonEncode({
            'id': 'x',
            'mime': 'video/mp4',
            'file_name': 'v.mp4',
            'size': 10,
            'local_path': '/m/x',
            'download_state': 'done',
          }),
        'send_message' => jsonEncode({'id': 'm1', 'kind': 'image', 'outgoing': true, 'view_once': 'unopened'}),
        _ => '',
      };
    }, const Stream.empty());
    final a = await backend.openViewOnce('m1');
    expect(a.localPath, '/m/x');
    expect(a.isVideo, isTrue);
    await backend.viewOnceDone('m1');
    final sent = await backend.sendMessage(
      conversationId: 'c1',
      attachments: [const OutgoingAttachment(path: '/p', fileName: 'p.jpg', mime: 'image/jpeg', kind: 'image')],
      viewOnce: true,
    );
    expect(sent.viewOnce, 'unopened');
    await backend.sendMessage(conversationId: 'c1', text: 'hi');
    expect(calls[0].$1, 'open_view_once');
    expect(calls[0].$2, {'message_id': 'm1'});
    expect(calls[1].$1, 'view_once_done');
    expect(calls[1].$2, {'message_id': 'm1'});
    expect((calls[2].$2 as Map)['view_once'], isTrue);
    expect((calls[3].$2 as Map).containsKey('view_once'), isFalse);
    unawaited(backend.dispose());
  });
}
