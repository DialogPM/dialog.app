import 'dart:convert';

import 'package:chatapp/app/globals.dart';
import 'package:chatapp/core/backend.dart';
import 'package:chatapp/core/models.dart';
import 'package:chatapp/features/chats/chat_list.dart';
import 'package:chatapp/features/conversation/conversation_page.dart';
import 'package:chatapp/l10n/i18n.dart';
import 'package:chatapp/theme/app_theme.dart';
import 'package:chatapp/theme/tokens.dart';
import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  testWidgets('disappearing messages are visible in the chat list and the chat header', (tester) async {
    container = ProviderContainer();
    backend = Backend((cmd, req) async {
      if (cmd == 'get_messages' || cmd.startsWith('list_') || cmd.endsWith('_messages')) return jsonEncode([]);
      return 'null';
    }, const Stream.empty());
    final conv = Conversation.fromJson({
      'id': 'c1', 'kind': 'direct', 'title': 'Ana', 'peer_account_id': 'acc-ana', 'is_admin': false, 'left': false,
      'unread_count': 0, 'archived': false, 'pinned': false, 'disappearing_seconds': 6 * 3600, 'blocked': false, 'updated_at': 1,
    });
    store.upsertConversation(conv);
    Widget app(Widget child) => UncontrolledProviderScope(
          container: container,
          child: MaterialApp(theme: buildTheme(ChatTokens.build(Direction.a, dark: false)), home: L10n(lang: 'ro', child: child)),
        );
    await tester.pumpWidget(app(ConversationPage(conversationId: conv.id)));
    await tester.pump(const Duration(milliseconds: 300));
    expect(find.byKey(const ValueKey('timer-chip')), findsOneWidget);
    expect(find.descendant(of: find.byKey(const ValueKey('timer-chip')), matching: find.text('6 ore')), findsOneWidget);
    expect(find.text('Mesaj temporar · 6 ore'), findsOneWidget);
    await tester.pumpWidget(app(Scaffold(body: ChatListItem(conv: conv))));
    await tester.pump();
    expect(find.byWidgetPredicate((w) => w is Semantics && w.properties.label == 'Mesaje temporare'), findsOneWidget);
    await tester.pumpWidget(Container());
    await tester.pump(const Duration(seconds: 2));
  });
}
