import 'package:chatapp/core/models.dart';
import 'package:chatapp/l10n/i18n.dart';
import 'package:chatapp/util/chat_export.dart';
import 'package:flutter_test/flutter_test.dart';

// The same cases as app/src/features/chats/exportChat.test.ts: both apps write the same text.

int at(int hour, int minute) => DateTime(2026, 11, 27, hour, minute).millisecondsSinceEpoch;
final now = DateTime(2026, 11, 28, 9, 30).millisecondsSinceEpoch;

Conversation conv({String kind = 'direct', String title = 'Ana Pop'}) =>
    Conversation.fromJson({'id': 'c1', 'kind': kind, 'title': title, 'peer_account_id': kind == 'direct' ? 'ana' : null});

var _seq = 0;

Message msg(Map<String, dynamic> over) => Message.fromJson({
      'id': 'm${++_seq}',
      'conversation_id': 'c1',
      'sender_account_id': 'ana',
      'sender_name': 'Ana Pop',
      'outgoing': false,
      'kind': 'text',
      'attachments': <dynamic>[],
      'status': 'read',
      'sent_at': at(14, 3),
      'received_at': at(14, 3),
      'deleted': false,
      'forwarded': false,
      ...over,
    });

Map<String, dynamic> file(Map<String, dynamic> over) => {
      'id': 'a${++_seq}',
      'mime': 'image/jpeg',
      'file_name': 'IMG_0001.jpg',
      'size': 1000,
      'local_path': '/media/x.jpg',
      'download_state': 'done',
      ...over,
    };

String body(String text) => text.split('\n').skip(4).join('\n');

void main() {
  test('a header and one dated line per message, oldest first', () {
    final r = buildTranscript(
      conv(),
      [
        msg({'text': 'Salut! Ce faci?'}),
        msg({'outgoing': true, 'sender_name': 'Me', 'text': 'Bine.\nTu?', 'sent_at': at(14, 5), 'edited_at': at(14, 6)}),
      ],
      'en',
      withMedia: false,
      now: now,
    );
    expect(
      r.text,
      [
        'Dialog · chat export: Ana Pop',
        'Exported on 2026-11-28 09:30',
        'Messages: 2',
        '',
        '[2026-11-27 14:03] Ana Pop: Salut! Ce faci?',
        '[2026-11-27 14:05] You: Bine.',
        'Tu? (edited)',
        '',
      ].join('\n'),
    );
    expect(r.count, 2);
    expect(r.media, isEmpty);
  });

  test('groups and bots: every sender is named, events are worded', () {
    final r = buildTranscript(
      conv(kind: 'group', title: 'Echipa'),
      [
        msg({'kind': 'system', 'sender_name': '', 'system': {'type': 'bot_started', 'actor_name': null, 'target_names': <String>[], 'value': null}}),
        msg({'sender_name': 'Bogdan', 'text': 'Bună', 'sent_at': at(14, 4)}),
        msg({
          'sender_name': 'Ana Pop',
          'text': 'ok',
          'sent_at': at(14, 5),
          'reply_to': {'id': 'x', 'sender_name': 'You', 'kind': 'image', 'text': null, 'thumbnail': null},
        }),
        msg({'sender_name': '', 'text': 'Sunny'}),
      ],
      'en',
      withMedia: false,
      now: now,
    );
    expect(
      body(r.text),
      [
        '[2026-11-27 14:03] ${translate('en', 'system.botStarted')}',
        '[2026-11-27 14:04] Bogdan: Bună',
        '[2026-11-27 14:05] Ana Pop: > You: Photo',
        'ok',
        '[2026-11-27 14:03] Echipa: Sunny',
        '',
      ].join('\n'),
    );
    expect(r.text.runes.any((c) => c >= 0x2066 && c <= 0x2069), isFalse);
  });

  test('with media: downloaded files get numbered names; the rest is only named', () {
    final photo = file({'file_name': '../../etc/passwd'});
    final doc = file({'mime': 'application/pdf', 'file_name': 'contract.pdf'});
    final unnamed = file({'mime': 'video/quicktime', 'file_name': ''});
    final missing = file({'mime': 'application/zip', 'file_name': 'big.zip', 'local_path': null, 'download_state': 'pending'});
    final m1 = msg({'kind': 'image', 'attachments': [photo, doc]});
    final m2 = msg({'kind': 'video', 'attachments': [unnamed, missing]});
    final r = buildTranscript(conv(), [m1, m2], 'en', withMedia: true, now: now);
    expect(r.media.map((m) => m.toJson()).toList(), [
      {'message_id': m1.id, 'attachment_id': photo['id'], 'name': '0001-_.._etc_passwd'},
      {'message_id': m1.id, 'attachment_id': doc['id'], 'name': '0002-contract.pdf'},
      {'message_id': m2.id, 'attachment_id': unnamed['id'], 'name': '0003-video.mov'},
    ]);
    expect(
      body(r.text),
      [
        '[2026-11-27 14:03] Ana Pop: [Photo: 0001-_.._etc_passwd]',
        '[Document: 0002-contract.pdf]',
        '[2026-11-27 14:03] Ana Pop: [Video: 0003-video.mov]',
        '[Document: big.zip (not included)]',
        '',
      ].join('\n'),
    );
    final text = buildTranscript(conv(), [m1], 'en', withMedia: false, now: now);
    expect(body(text.text), '[2026-11-27 14:03] Ana Pop: [Photo]\n[Document: contract.pdf]\n');
    expect(text.media, isEmpty);
  });

  test('no more than the conversation shows', () {
    final r = buildTranscript(
      conv(),
      [
        msg({'deleted': true}),
        msg({'outgoing': true, 'deleted': true}),
        msg({'kind': 'image', 'view_once': 'opened', 'attachments': [file({'local_path': null})]}),
        msg({'kind': 'image', 'attachments': [file({'local_path': null, 'download_state': 'expired', 'expired_at': at(10, 0), 'file_name': ''})]}),
        msg({'text': 'gone already', 'expires_at': now - 1}),
        msg({'text': 'still here', 'expires_at': now + 60000}),
      ],
      'en',
      withMedia: true,
      now: now,
    );
    expect(r.media, isEmpty);
    expect(r.count, 5);
    expect(r.text.contains('gone already'), isFalse);
    expect(body(r.text).split('\n').length, 6);
    expect(body(r.text), contains('Ana Pop: still here'));
    expect(body(r.text), contains('[View-once photo]'));
  });

  test('locations, contact cards, forwarded messages, file names', () {
    final r = buildTranscript(
      conv(),
      [
        msg({'kind': 'location', 'location': {'lat': 44.4268, 'lng': 26.1025, 'name': 'Bucharest', 'address': null}}),
        msg({'kind': 'contact', 'contacts': [{'account_id': 'x', 'number': '1234567890123456', 'name': 'Dan'}]}),
        msg({'text': 'see this', 'forwarded': true}),
      ],
      'en',
      withMedia: false,
      now: now,
    );
    expect(
      body(r.text),
      [
        '[2026-11-27 14:03] Ana Pop: [Location: Bucharest, 44.4268, 26.1025]',
        '[2026-11-27 14:03] Ana Pop: [Contact: Dan, 1234 5678 9012 3456]',
        '[2026-11-27 14:03] Ana Pop: (Forwarded)',
        'see this',
        '',
      ].join('\n'),
    );
    expect(exportTimestamp(DateTime(2026, 1, 5, 7, 9).millisecondsSinceEpoch), '2026-01-05 07:09');
    expect(safeFileName('a/b\\c:d*e?f"g<h>i|j'), 'a_b_c_d_e_f_g_h_i_j');
    expect(safeFileName('  .hidden  '), 'hidden');
    expect(safeFileName('...', 'photo.jpg'), 'photo.jpg');
    final long = safeFileName('${'x' * 300}.jpeg');
    expect(long.length, 100);
    expect(long.endsWith('.jpeg'), isTrue);
    expect(exportBaseName('Ana / Pop?'), 'Dialog - Ana _ Pop_');
  });
}
