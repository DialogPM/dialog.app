import 'dart:convert';

import 'package:chatapp/app/globals.dart';
import 'package:chatapp/core/backend.dart';
import 'package:chatapp/core/models.dart';
import 'package:chatapp/features/bots/bot_widgets.dart';
import 'package:chatapp/features/bots/bots_page.dart';
import 'package:chatapp/l10n/i18n.dart';
import 'package:chatapp/theme/app_theme.dart';
import 'package:chatapp/theme/tokens.dart';
import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_test/flutter_test.dart';

Json _conv(String state, {List<Json> commands = const [], bool deleted = false}) => {
      'id': 'c1', 'kind': 'direct', 'title': 'Weather', 'peer_account_id': 'b1', 'is_admin': false, 'left': false,
      'unread_count': 0, 'archived': false, 'pinned': false, 'disappearing_seconds': 0, 'blocked': false, 'updated_at': 1,
      'bot': {'id': 'b1', 'username': 'weather_bot', 'name': 'Weather', 'about': 'Rain alerts', 'commands': commands, 'state': state, 'mine': false, 'deleted': deleted},
    };

Widget _app(Widget child) => UncontrolledProviderScope(
      container: container,
      child: MaterialApp(
        theme: buildTheme(ChatTokens.build(Direction.a, dark: false)),
        home: L10n(lang: 'en', child: child),
      ),
    );

Widget _chat(Conversation c) => Scaffold(
      body: Column(children: [
        Expanded(child: c.bot!.state == 'none' ? BotLanding(conv: c, bot: c.bot!) : const SizedBox()),
        BotFooter(conv: c, bot: c.bot!),
      ]),
    );

void main() {
  test('bot names and addresses', () {
    expect(looksLikeBotName('weather_bot'), isTrue);
    expect(looksLikeBotName('maria.01'), isFalse);
    expect(looksLikeBotName('bot'), isFalse);
    expect(botSlug('Alarmă casă'), 'alarma_casa');
    expect(botSlug('  Server alerts bot '), 'server_alerts');
    expect(botSlug('9 lives'), 'lives');
  });

  test('a conversation says when its peer is a bot', () {
    final c = Conversation.fromJson(_conv('active', commands: [
      {'command': 'today', 'description': 'Forecast'},
    ]));
    expect(c.bot!.username, 'weather_bot');
    expect(c.bot!.active, isTrue);
    expect(c.bot!.commands.single.command, 'today');
    final plain = Conversation.fromJson(Map.of(_conv('active'))..remove('bot'));
    expect(plain.bot, isNull);
  });

  testWidgets('a bot that was not started shows what it is and starts on request', (tester) async {
    final calls = <String>[];
    container = ProviderContainer();
    backend = Backend((cmd, req) async {
      calls.add(cmd);
      if (cmd == 'bot_start') return jsonEncode(_conv('active'));
      return 'null';
    }, const Stream.empty());
    final c = Conversation.fromJson(_conv('none'));
    await tester.pumpWidget(_app(_chat(c)));
    expect(find.text('@weather_bot'), findsOneWidget);
    expect(find.text('BOT'), findsOneWidget);
    expect(find.text('Messages with bots are not end-to-end encrypted'), findsOneWidget);
    expect(calls, isEmpty, reason: 'looking at a bot tells it nothing');
    await tester.tap(find.byKey(const ValueKey('bot-start')));
    await tester.pumpAndSettle();
    expect(calls, ['bot_start']);
    expect(appData.conversations['c1']!.bot!.state, 'active');
  });

  testWidgets('a started bot without commands can be muted or stopped', (tester) async {
    final calls = <String>[];
    container = ProviderContainer();
    backend = Backend((cmd, req) async {
      calls.add(cmd);
      if (cmd == 'bot_stop') return jsonEncode(_conv('stopped'));
      return 'null';
    }, const Stream.empty());
    await tester.pumpWidget(_app(_chat(Conversation.fromJson(_conv('active')))));
    expect(find.text('Mute'), findsOneWidget);
    expect(find.byKey(const ValueKey('bot-input')), findsNothing);
    await tester.tap(find.byKey(const ValueKey('bot-stop')));
    await tester.pumpAndSettle();
    expect(calls, isEmpty, reason: 'stopping asks first');
    await tester.tap(find.text('Stop the bot').last);
    await tester.pumpAndSettle();
    expect(calls, ['bot_stop']);
    expect(appData.conversations['c1']!.bot!.state, 'stopped');
  });

  testWidgets('commands are offered after a slash and sent as text', (tester) async {
    final sent = <Map<String, dynamic>>[];
    container = ProviderContainer();
    backend = Backend((cmd, req) async {
      if (cmd == 'send_message') sent.add(jsonDecode(req) as Map<String, dynamic>);
      return 'null';
    }, const Stream.empty());
    final c = Conversation.fromJson(_conv('active', commands: [
      {'command': 'today', 'description': 'Forecast for today'},
      {'command': 'tomorrow', 'description': 'Forecast for tomorrow'},
      {'command': 'alerts', 'description': 'Warnings in force'},
    ]));
    await tester.pumpWidget(_app(_chat(c)));
    expect(find.text('/today'), findsNothing);
    await tester.enterText(find.byKey(const ValueKey('bot-input')), '/to');
    await tester.pump();
    expect(find.text('/today'), findsOneWidget);
    expect(find.text('/tomorrow'), findsOneWidget);
    expect(find.text('/alerts'), findsNothing);
    await tester.tap(find.text('/tomorrow'));
    await tester.pump();
    expect(sent.single['text'], '/tomorrow');
    expect(sent.single['conversation_id'], 'c1');
  });

  testWidgets('buttons under a bot message tell the bot or ask before opening a link', (tester) async {
    final calls = <(String, Map<String, dynamic>?)>[];
    container = ProviderContainer();
    backend = Backend((cmd, req) async {
      calls.add((cmd, jsonDecode(req) as Map<String, dynamic>?));
      return 'null';
    }, const Stream.empty());
    final m = Message.fromJson({
      'id': 'm1', 'conversation_id': 'c1', 'sender_account_id': 'b1', 'sender_name': 'Weather', 'outgoing': false, 'kind': 'text',
      'text': 'Motion at the door', 'status': 'read', 'sent_at': 1, 'received_at': 1, 'deleted': false, 'forwarded': false,
      'buttons': [
        [
          {'text': 'Ignore', 'data': 'ignore:7'},
          {'text': 'Live', 'url': 'https://example.org/cam'},
        ],
        [
          {'text': 'broken'},
        ],
      ],
    });
    expect(m.buttons.length, 1, reason: 'a button with neither data nor a link is not shown');
    await tester.pumpWidget(_app(Scaffold(body: Center(child: SizedBox(width: 300, child: BotButtons(message: m))))));
    await tester.tap(find.text('Ignore'));
    await tester.pump();
    expect(calls.single.$1, 'bot_press');
    expect(calls.single.$2, {'message_id': 'm1', 'data': 'ignore:7'});
    await tester.pump(const Duration(seconds: 2));
    await tester.tap(find.text('Live'));
    await tester.pumpAndSettle();
    expect(find.text('https://example.org/cam'), findsOneWidget, reason: 'the address is shown before anything opens');
    expect(calls.length, 1);
    await tester.tap(find.text('Cancel'));
    await tester.pumpAndSettle();
  });

  testWidgets('a stopped or deleted bot says so', (tester) async {
    container = ProviderContainer();
    backend = Backend((cmd, req) async => 'null', const Stream.empty());
    await tester.pumpWidget(_app(_chat(Conversation.fromJson(_conv('stopped')))));
    expect(find.text('You stopped this bot. It can no longer write to you.'), findsOneWidget);
    expect(find.byKey(const ValueKey('bot-start')), findsOneWidget);
    await tester.pumpWidget(_app(_chat(Conversation.fromJson(_conv('stopped', deleted: true)))));
    expect(find.text('This bot was deleted by its owner.'), findsOneWidget);
    expect(find.byKey(const ValueKey('bot-start')), findsNothing);
  });
}
