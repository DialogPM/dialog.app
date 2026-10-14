import 'dart:convert';

import 'package:chatapp/app/globals.dart';
import 'package:chatapp/core/backend.dart';
import 'package:chatapp/core/models.dart';
import 'package:chatapp/features/chats/starred_page.dart';
import 'package:chatapp/features/conversation/pinned_bar.dart';
import 'package:chatapp/l10n/i18n.dart';
import 'package:chatapp/theme/app_theme.dart';
import 'package:chatapp/theme/tokens.dart';
import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_test/flutter_test.dart';

Json _msg(String id, String text, {int? pinnedUntil, bool starred = false}) => {
      'id': id,
      'conversation_id': 'c1',
      'sender_account_id': 'me',
      'sender_name': 'You',
      'outgoing': true,
      'kind': 'text',
      'text': text,
      'status': 'sent',
      'sent_at': 1,
      'received_at': 1,
      'deleted': false,
      'forwarded': false,
      'pinned_until': pinnedUntil,
      'starred': starred,
    };

Widget _app(Widget child) => UncontrolledProviderScope(
      container: container,
      child: MaterialApp(
        theme: buildTheme(ChatTokens.build(Direction.a, dark: false)),
        home: L10n(lang: 'en', child: child),
      ),
    );

final _conv = Conversation.fromJson({
  'id': 'c1', 'kind': 'direct', 'title': 'Bogdana', 'is_admin': false, 'left': false, 'unread_count': 0,
  'archived': false, 'pinned': false, 'disappearing_seconds': 0, 'blocked': false, 'updated_at': 1,
});

void main() {
  testWidgets('pinned bar cycles through pins and unpins', (tester) async {
    final calls = <(String, Map<String, dynamic>?)>[];
    var pins = [_msg('m3', 'third', pinnedUntil: 9e15.toInt()), _msg('m2', 'second', pinnedUntil: 9e15.toInt())];
    container = ProviderContainer();
    backend = Backend((cmd, req) async {
      calls.add((cmd, jsonDecode(req) as Map<String, dynamic>?));
      if (cmd == 'pinned_messages') return jsonEncode(pins);
      if (cmd == 'pin_message') {
        pins = pins.where((p) => p['id'] != (jsonDecode(req) as Map)['message_id']).toList();
        return jsonEncode(_msg('m3', 'third'));
      }
      return 'null';
    }, const Stream.empty());
    store.upsertConversation(_conv);
    await tester.pumpWidget(_app(Scaffold(body: Column(children: [PinnedBar(conv: _conv)]))));
    await tester.pumpAndSettle();
    expect(find.text('Pinned message 1 of 2'), findsOneWidget);
    expect(find.text('third'), findsOneWidget);
    await tester.tap(find.text('third'));
    await tester.pumpAndSettle();
    expect(appData.highlightId, 'm3');
    expect(find.text('Pinned message 2 of 2'), findsOneWidget);
    expect(find.text('second'), findsOneWidget);
    // Unpin the shown one; the core then reports pins_changed.
    await tester.tap(find.byTooltip('Unpin'));
    await tester.pumpAndSettle();
    expect(calls.where((c) => c.$1 == 'pin_message').single.$2, {'message_id': 'm2', 'seconds': null});
    store.set((d) => d.copyWith(pinsVersion: d.pinsVersion + 1));
    await tester.pumpAndSettle();
    expect(find.text('Pinned message'), findsOneWidget);
  });

  testWidgets('starred page lists stars and unstars', (tester) async {
    var starred = [_msg('m1', 'remember this', starred: true)];
    container = ProviderContainer();
    backend = Backend((cmd, req) async {
      if (cmd == 'starred_messages') return jsonEncode(starred);
      if (cmd == 'star_message') {
        starred = [];
        return jsonEncode(_msg('m1', 'remember this'));
      }
      return 'null';
    }, const Stream.empty());
    store.upsertConversation(_conv);
    await tester.pumpWidget(_app(const StarredPage()));
    await tester.pumpAndSettle();
    expect(find.textContaining('remember this'), findsOneWidget);
    await tester.tap(find.byTooltip('Unstar'));
    await tester.pumpAndSettle();
    expect(find.text('No starred messages'), findsOneWidget);
  });
}
