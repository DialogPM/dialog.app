import 'dart:async';
import 'dart:convert';

import 'package:chatapp/app/globals.dart';
import 'package:chatapp/core/backend.dart';
import 'package:chatapp/core/models.dart';
import 'package:chatapp/features/chats/chat_list.dart' show NoticesBell;
import 'package:chatapp/features/notices/notices_page.dart';
import 'package:chatapp/l10n/i18n.dart';
import 'package:chatapp/state/notices.dart';
import 'package:chatapp/theme/app_theme.dart';
import 'package:chatapp/theme/tokens.dart';
import 'package:chatapp/widgets/official_badge.dart';
import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_test/flutter_test.dart';

Json _notice(int id, {String title = 'T', String body = 'B', int created = 1, bool read = false}) =>
    {'id': id, 'title': title, 'body': body, 'created_at': created, 'received_at': created + 1, 'read': read};

Widget _app(Widget child, {String lang = 'en', TextDirection dir = TextDirection.ltr}) => UncontrolledProviderScope(
      container: container,
      child: MaterialApp(
        theme: buildTheme(ChatTokens.build(Direction.a, dark: false)),
        home: L10n(lang: lang, child: Directionality(textDirection: dir, child: child)),
      ),
    );

/// A core that does not know the notice commands (older version).
Backend _oldCore() => Backend((cmd, req) async => throw Exception('unknown_command: $cmd'), const Stream.empty());

void main() {
  group('official badge in the models', () {
    test('Contact, Conversation, FoundAccount and Profile read the label', () {
      expect(Contact.fromJson({'account_id': 'a', 'official': 'Dialog Support'}).official, 'Dialog Support');
      expect(Conversation.fromJson({'id': 'c', 'kind': 'direct', 'official': ' Dialog Support '}).official, 'Dialog Support');
      expect(FoundAccount.fromJson({'account_id': 'a', 'official': 'Dialog Support'}).official, 'Dialog Support');
      expect(Profile.fromJson({'name': 'Me', 'official': 'Dialog Support'}).official, 'Dialog Support');
    });

    test('absent, null, blank or non-string labels from an older core mean no badge', () {
      for (final v in <Object?>[null, '', '   ', 1, true, {'label': 'x'}]) {
        expect(Contact.fromJson({'account_id': 'a', 'official': v}).official, isNull, reason: '$v');
        expect(Conversation.fromJson({'id': 'c', 'official': v}).official, isNull, reason: '$v');
        expect(FoundAccount.fromJson({'account_id': 'a', 'official': v}).official, isNull, reason: '$v');
        expect(Profile.fromJson({'official': v}).official, isNull, reason: '$v');
      }
      expect(Contact.fromJson({'account_id': 'a'}).official, isNull);
      expect(Profile.fromJson({}).official, isNull);
    });

    test('a group never carries a badge', () {
      expect(Conversation.fromJson({'id': 'g', 'kind': 'group', 'official': 'Dialog Support'}).official, isNull);
    });

    test('a conversation falls back to the contact record', () {
      final conv = Conversation.fromJson({'id': 'c', 'kind': 'direct', 'peer_account_id': 'p'});
      final contacts = {'p': Contact.fromJson({'account_id': 'p', 'official': 'Support'})};
      expect(conversationOfficial(conv, contacts), 'Support');
      expect(conversationOfficial(conv, const {}), isNull);
      final group = Conversation.fromJson({'id': 'g', 'kind': 'group', 'peer_account_id': 'p'});
      expect(conversationOfficial(group, contacts), isNull);
    });
  });

  group('notices in the models', () {
    test('Notice parses all fields', () {
      final n = Notice.fromJson(_notice(7, title: 'Maintenance', body: 'Tonight\n22:00', created: 100, read: true));
      expect(n.id, 7);
      expect(n.title, 'Maintenance');
      expect(n.body, 'Tonight\n22:00');
      expect(n.createdAt, 100);
      expect(n.receivedAt, 101);
      expect(n.read, isTrue);
    });

    test('Notice tolerates missing and null fields', () {
      final n = Notice.fromJson({'id': 3, 'title': null});
      expect(n.title, '');
      expect(n.body, '');
      expect(n.createdAt, 0);
      expect(n.read, isFalse);
    });

    test('NoticeList with and without the unread count', () {
      final l = NoticeList.fromJson(jsonDecode(jsonEncode({'notices': [_notice(2), _notice(1, read: true)], 'unread': 1})) as Json);
      expect(l.notices.map((n) => n.id), [2, 1]);
      expect(l.unread, 1);
      expect(NoticeList.fromJson({'notices': [_notice(2), _notice(1)]}).unread, 2);
      expect(NoticeList.fromJson({}).notices, isEmpty);
    });

    test('notice events are parsed from the core stream', () async {
      final raw = StreamController<String>();
      final b = Backend((cmd, req) async => 'null', raw.stream);
      final notice = b.onNotice.first;
      final count = b.onNoticesChanged.first;
      raw.add(jsonEncode({'event': 'notice', 'payload': _notice(9, title: 'Update required')}));
      raw.add(jsonEncode({'event': 'notices_changed', 'payload': {'unread': 4}}));
      expect((await notice).title, 'Update required');
      expect(await count, 4);
      await raw.close();
      await b.dispose();
    });
  });

  group('unread count', () {
    NoticesState loaded(List<Json> list, int unread) => NoticesState.loaded(NoticeList.fromJson({'notices': list, 'unread': unread}));

    test('loaded lists are newest first', () {
      final s = loaded([_notice(1, created: 10), _notice(2, created: 30), _notice(3, created: 20)], 3);
      expect(s.supported, isTrue);
      expect(s.notices.map((n) => n.id), [2, 3, 1]);
      expect(s.unread, 3);
    });

    test('a new notice counts once and goes on top', () {
      var s = loaded([_notice(1, created: 10, read: true)], 0);
      s = s.withNotice(Notice.fromJson(_notice(2, created: 20)));
      expect(s.unread, 1);
      expect(s.notices.first.id, 2);
      // The same notice again (sync from the core) is not counted twice.
      s = s.withNotice(Notice.fromJson(_notice(2, created: 20)));
      expect(s.unread, 1);
      expect(s.notices.length, 2);
      // Read on another device: the core's count wins.
      s = s.withUnread(0);
      expect(s.unread, 0);
      expect(s.withUnread(-3).unread, 0);
    });

    test('a notice arriving before the list makes notices supported', () {
      final s = NoticesState.none.withNotice(Notice.fromJson(_notice(1)));
      expect(s.supported, isTrue);
      expect(s.unread, 1);
    });

    test('mark read and delete', () {
      final s = loaded([_notice(1, created: 10), _notice(2, created: 20, read: true)], 1);
      final r = s.markedRead();
      expect(r.unread, 0);
      expect(r.notices.every((n) => n.read), isTrue);
      expect(s.without(1).unread, 0);
      expect(s.without(2).unread, 1);
      expect(s.without(99), same(s));
    });

    test('an older core shows no notices and no error', () async {
      container = ProviderContainer();
      backend = _oldCore();
      noticesState.value = NoticesState.loaded(const NoticeList(notices: [], unread: 2));
      await loadNotices();
      expect(noticesState.value.supported, isFalse);
      expect(noticesState.value.notices, isEmpty);
      expect(noticesState.value.unread, 0);
    });

    test('load, mark read and delete call the core', () async {
      final calls = <(String, Object?)>[];
      container = ProviderContainer();
      backend = Backend((cmd, req) async {
        calls.add((cmd, jsonDecode(req)));
        return switch (cmd) {
          'list_notices' => jsonEncode({'notices': [_notice(2, created: 20), _notice(1, created: 10)], 'unread': 2}),
          'mark_notices_read' => jsonEncode({'unread': 0}),
          _ => '{}',
        };
      }, const Stream.empty());
      await loadNotices();
      expect(noticesState.value.unread, 2);
      await markNoticesRead();
      expect(noticesState.value.unread, 0);
      await deleteNotice(1);
      expect(noticesState.value.notices.map((n) => n.id), [2]);
      expect(calls.map((c) => c.$1), ['list_notices', 'mark_notices_read', 'delete_notice']);
      expect(calls.last.$2, {'id': 1});
    });
  });

  group('widgets', () {
    testWidgets('badge renders nothing without a label', (tester) async {
      container = ProviderContainer();
      for (final label in [null, '', '  ']) {
        await tester.pumpWidget(_app(Row(children: [const Text('Ana'), OfficialBadge(label)])));
        expect(find.byType(OfficialSeal), findsNothing, reason: '$label');
      }
    });

    testWidgets('badge exposes its semantic label', (tester) async {
      container = ProviderContainer();
      final handle = tester.ensureSemantics();
      await tester.pumpWidget(_app(const Row(children: [Text('Ana'), OfficialBadge('Dialog Support')])));
      expect(find.byType(OfficialSeal), findsOneWidget);
      expect(find.bySemanticsLabel('Official account · Dialog Support'), findsOneWidget);
      await tester.pumpWidget(_app(const Row(children: [Text('Ana'), OfficialBadge('Dialog Support')]), lang: 'ro'));
      expect(find.bySemanticsLabel('Cont oficial · Dialog Support'), findsOneWidget);
      handle.dispose();
    });

    testWidgets('badge follows the name in right-to-left layouts', (tester) async {
      container = ProviderContainer();
      const row = Row(mainAxisSize: MainAxisSize.min, children: [Text('Ana'), OfficialBadge('Support')]);
      await tester.pumpWidget(_app(const Center(child: row), lang: 'ar', dir: TextDirection.rtl));
      expect(tester.getTopLeft(find.byType(OfficialSeal)).dx, lessThan(tester.getTopLeft(find.text('Ana')).dx));
      await tester.pumpWidget(_app(const Center(child: row)));
      expect(tester.getTopLeft(find.byType(OfficialSeal)).dx, greaterThan(tester.getTopLeft(find.text('Ana')).dx));
    });

    testWidgets('bell is hidden on an older core and shows a dot for unread notices', (tester) async {
      container = ProviderContainer();
      noticesState.value = NoticesState.none;
      await tester.pumpWidget(_app(const Scaffold(body: Row(children: [NoticesBell()]))));
      expect(find.byTooltip('Server notices'), findsNothing);
      noticesState.value = NoticesState.loaded(NoticeList(notices: [Notice.fromJson(_notice(1))], unread: 1));
      await tester.pump();
      expect(find.byTooltip('Server notices · New'), findsOneWidget);
      expect(find.byKey(const ValueKey('notices-dot')), findsOneWidget);
      noticesState.value = noticesState.value.withUnread(0);
      await tester.pump();
      expect(find.byTooltip('Server notices'), findsOneWidget);
      expect(find.byKey(const ValueKey('notices-dot')), findsNothing);
    });

    testWidgets('notices page: plain text, read on open, empty state', (tester) async {
      container = ProviderContainer();
      var list = [_notice(1, title: 'Maintenance', body: '**Tonight**\nfrom 22:00 https://x.example', created: 10)];
      final calls = <String>[];
      backend = Backend((cmd, req) async {
        calls.add(cmd);
        return switch (cmd) {
          'list_notices' => jsonEncode({'notices': list, 'unread': list.where((n) => n['read'] != true).length}),
          'mark_notices_read' => jsonEncode({'unread': 0}),
          _ => '{}',
        };
      }, const Stream.empty());
      noticesState.value = NoticesState.none;
      await tester.pumpWidget(_app(const NoticesPage()));
      await tester.pumpAndSettle();
      expect(find.textContaining('nobody can reply'), findsOneWidget);
      expect(find.text('Maintenance'), findsOneWidget);
      // Shown exactly as written: no markdown, line break kept.
      expect(find.text('**Tonight**\nfrom 22:00 https://x.example'), findsOneWidget);
      expect(calls, containsAllInOrder(['list_notices', 'mark_notices_read']));
      expect(noticesState.value.unread, 0);
      expect(noticesPageOpen, isTrue);

      list = [];
      await tester.pumpWidget(_app(const SizedBox()));
      expect(noticesPageOpen, isFalse);
      await tester.pumpWidget(_app(const NoticesPage()));
      await tester.pumpAndSettle();
      expect(find.textContaining('No notices'), findsOneWidget);
    });
  });
}
