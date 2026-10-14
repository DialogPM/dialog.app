import 'dart:async';
import 'dart:convert';

import 'package:chatapp/app/globals.dart';
import 'package:chatapp/core/backend.dart';
import 'package:chatapp/core/models.dart';
import 'package:chatapp/features/conversation/composer.dart';
import 'package:chatapp/features/conversation/message_bubble.dart';
import 'package:chatapp/l10n/i18n.dart';
import 'package:chatapp/theme/app_theme.dart';
import 'package:chatapp/theme/tokens.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_test/flutter_test.dart';

const _bob = '22222222-2222-4222-8222-222222222222';
const _cris = '33333333-3333-4333-8333-333333333333';

Json _message(Json req) => {
      'id': 'm1',
      'conversation_id': req['conversation_id'],
      'sender_account_id': 'me',
      'sender_name': 'You',
      'outgoing': true,
      'kind': 'text',
      'text': req['text'],
      'status': 'sending',
      'sent_at': 1,
      'received_at': 1,
      'deleted': false,
      'forwarded': false,
      'mentions': req['mentions'] ?? [],
    };

void main() {
  testWidgets('@ suggestions insert the member and the mention is sent', (tester) async {
    final calls = <(String, Json)>[];
    container = ProviderContainer();
    backend = Backend((cmd, reqJson) async {
      final req = (jsonDecode(reqJson) as Map<String, dynamic>?) ?? {};
      calls.add((cmd, req));
      if (cmd == 'send_message') return jsonEncode(_message(req));
      return 'null';
    }, const Stream.empty());
    final conv = Conversation.fromJson({
      'id': 'g1', 'kind': 'group', 'title': 'Team', 'is_admin': false, 'left': false, 'unread_count': 0,
      'archived': false, 'pinned': false, 'disappearing_seconds': 0, 'blocked': false, 'updated_at': 1,
    });
    store.upsertConversation(conv);
    store.upsertGroup(GroupInfo.fromJson({
      'conversation_id': 'g1', 'group_id': 'gid', 'name': 'Team', 'member_count': 3, 'my_role': 'member',
      'members': [
        {'account_id': 'me', 'number': '1', 'display_name': 'Me', 'role': 'member', 'is_me': true},
        {'account_id': _bob, 'number': '2', 'display_name': 'Bogdana', 'role': 'admin', 'is_me': false},
        {'account_id': _cris, 'number': '3', 'display_name': 'Cristi', 'role': 'member', 'is_me': false},
      ],
    }));
    await tester.pumpWidget(UncontrolledProviderScope(
      container: container,
      child: MaterialApp(
        theme: buildTheme(ChatTokens.build(Direction.a, dark: false)),
        home: L10n(lang: 'en', child: Scaffold(body: Column(children: [const Spacer(), Composer(conv: conv)]))),
      ),
    ));
    await tester.enterText(find.byType(TextField), 'Hi @bo');
    await tester.pump();
    expect(find.byKey(const ValueKey('mention-suggestions')), findsOneWidget);
    expect(find.text('Cristi'), findsNothing);
    await tester.tap(find.text('Bogdana'));
    await tester.pump();
    final field = tester.widget<TextField>(find.byType(TextField));
    expect(field.controller!.text, 'Hi @Bogdana ');
    expect(find.byKey(const ValueKey('mention-suggestions')), findsNothing);
    // Typing more keeps the mention.
    field.controller!.value = const TextEditingValue(text: '  Hi @Bogdana *look*', selection: TextSelection.collapsed(offset: 20));
    await tester.pump();
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await tester.pump(const Duration(milliseconds: 50));
    final send = calls.where((c) => c.$1 == 'send_message').single.$2;
    expect(send['text'], 'Hi @Bogdana *look*');
    expect(send['mentions'], [
      {'account_id': _bob, 'start': 3, 'length': 8},
    ]);
    await tester.pumpWidget(Container());
    await tester.pump(const Duration(seconds: 1));
  });

  testWidgets('bubbles render mentions and formatting', (tester) async {
    container = ProviderContainer();
    backend = Backend((cmd, reqJson) async => 'null', const Stream.empty());
    store.upsertMessage(Message.fromJson({
      ..._message({'conversation_id': 'g1', 'text': 'Hi @Bogdana *look* _here_', 'mentions': [{'account_id': _bob, 'start': 3, 'length': 8}]}),
      'outgoing': false,
      'sender_account_id': _cris,
      'sender_name': 'Cristi',
      'status': 'delivered',
    }));
    await tester.pumpWidget(UncontrolledProviderScope(
      container: container,
      child: MaterialApp(
        theme: buildTheme(ChatTokens.build(Direction.a, dark: false)),
        home: L10n(
          lang: 'en',
          child: const Scaffold(body: MessageRow(id: 'm1', first: true, group: true, selecting: false, selected: false, highlighted: false)),
        ),
      ),
    ));
    final rich = tester.widgetList<RichText>(find.byType(RichText)).firstWhere((r) => r.text.toPlainText().contains('Bogdana'));
    // Markers are hidden, the text is intact.
    expect(rich.text.toPlainText(), 'Hi @Bogdana look here');
    // Leaf text with its effective style (parent styles merged).
    final leaves = <(String, TextStyle)>[];
    void walk(InlineSpan span, TextStyle style) {
      if (span is! TextSpan) return;
      final st = style.merge(span.style);
      if (span.text != null) leaves.add((span.text!, st));
      for (final c in span.children ?? const <InlineSpan>[]) {
        walk(c, st);
      }
    }

    walk(rich.text, const TextStyle());
    TextStyle styleOf(String text) => leaves.firstWhere((l) => l.$1 == text).$2;
    expect(styleOf('@Bogdana').fontWeight, FontWeight.w600);
    expect(styleOf('look').fontWeight, FontWeight.w700);
    expect(styleOf('here').fontStyle, FontStyle.italic);
    expect(styleOf('Hi ').fontWeight, isNot(FontWeight.w700));
  });

  testWidgets('a chat with disappearing messages says so where the user types', (tester) async {
    container = ProviderContainer();
    backend = Backend((cmd, reqJson) async => 'null', const Stream.empty());
    Conversation conv(int seconds) => Conversation.fromJson({
          'id': 'd1', 'kind': 'direct', 'title': 'Ana', 'is_admin': false, 'left': false, 'unread_count': 0,
          'archived': false, 'pinned': false, 'disappearing_seconds': seconds, 'blocked': false, 'updated_at': 1,
        });
    Widget app(Conversation c) => UncontrolledProviderScope(
          container: container,
          child: MaterialApp(
            theme: buildTheme(ChatTokens.build(Direction.a, dark: false)),
            home: L10n(lang: 'en', child: Scaffold(body: Column(children: [const Spacer(), Composer(key: ValueKey(c.disappearingSeconds), conv: c)]))),
          ),
        );
    await tester.pumpWidget(app(conv(6 * 3600)));
    expect(find.text('Disappearing message · 6 hours'), findsOneWidget);
    await tester.pumpWidget(app(conv(0)));
    expect(find.text('Type a message'), findsOneWidget);
  });
}
