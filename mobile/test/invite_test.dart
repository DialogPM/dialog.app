import 'dart:convert';

import 'package:chatapp/app/globals.dart';
import 'package:chatapp/core/backend.dart';
import 'package:chatapp/core/models.dart';
import 'package:chatapp/features/groups/invite_pages.dart';
import 'package:chatapp/l10n/i18n.dart';
import 'package:chatapp/theme/app_theme.dart';
import 'package:chatapp/theme/tokens.dart';
import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_test/flutter_test.dart';

const _link = 'chatapp://join/AbCdEfGhIjKlMnOpQrStUv';

Widget _app(Widget child) => UncontrolledProviderScope(
      container: container,
      child: MaterialApp(theme: buildTheme(ChatTokens.build(Direction.a, dark: false)), home: L10n(lang: 'en', child: child)),
    );

Json _group({String? link, bool approval = false, int requests = 0}) => {
      'conversation_id': 'g1',
      'group_id': 'gid',
      'name': 'Book club',
      'member_count': 2,
      'my_role': 'admin',
      'members': [],
      'invite_link': link,
      'invite_approval': approval,
      'join_requests': requests,
    };

void main() {
  testWidgets('joining with a link: preview, ask, cancel', (tester) async {
    final calls = <String>[];
    container = ProviderContainer();
    backend = Backend((cmd, req) async {
      calls.add(cmd);
      switch (cmd) {
        case 'preview_group_invite':
          return jsonEncode({
            'group_id': 'gid', 'name': 'Book club', 'description': 'We read together', 'member_count': 5,
            'approval': true, 'is_member': false, 'requested': false, 'conversation_id': null,
          });
        case 'join_group_by_link':
          return jsonEncode({'status': 'requested', 'group_id': 'gid', 'conversation_id': null});
      }
      return 'null';
    }, const Stream.empty());
    await tester.pumpWidget(_app(const JoinLinkPage()));
    await tester.enterText(find.byType(TextField), 'not a link');
    await tester.pumpAndSettle();
    expect(find.textContaining('chatapp://join/'), findsWidgets); // the hint about the format
    await tester.enterText(find.byType(TextField), 'Come: $_link');
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('invite-name')), findsOneWidget);
    expect(find.text('We read together'), findsOneWidget);
    expect(find.text('Group · 5 members'), findsOneWidget);
    await tester.tap(find.text('Ask to join'));
    await tester.pumpAndSettle();
    expect(find.text('Request sent'), findsOneWidget);
    await tester.tap(find.text('Cancel request'));
    await tester.pumpAndSettle();
    expect(calls, containsAllInOrder(['preview_group_invite', 'join_group_by_link', 'cancel_join_request']));
    expect(find.text('Ask to join'), findsOneWidget);
  });

  testWidgets('admin creates the link and approves a request', (tester) async {
    final calls = <(String, Map<String, dynamic>?)>[];
    var requests = [
      {'account_id': 'acc-2', 'name': 'Cristina Pop', 'avatar_path': null, 'requested_at': 1},
    ];
    container = ProviderContainer();
    backend = Backend((cmd, req) async {
      calls.add((cmd, jsonDecode(req) as Map<String, dynamic>?));
      switch (cmd) {
        case 'set_group_invite':
          return jsonEncode(_group(link: _link, approval: true, requests: 1));
        case 'group_join_requests':
          return jsonEncode(requests);
        case 'approve_join_request':
          requests = [];
          return jsonEncode(_group(link: _link, approval: true));
      }
      return 'null';
    }, const Stream.empty());
    store.upsertGroup(GroupInfo.fromJson(_group()));
    await tester.pumpWidget(_app(const InviteLinkPage(conversationId: 'g1')));
    await tester.tap(find.text('Approve new members'));
    await tester.pump();
    await tester.tap(find.text('Create link'));
    await tester.pumpAndSettle();
    expect(calls.last.$2, {'conversation_id': 'g1', 'enabled': true, 'approval': true, 'reset': false});
    expect(find.text(_link), findsOneWidget);

    // The banner and the requests page.
    await tester.pumpWidget(_app(const Scaffold(body: JoinRequestsBanner(conversationId: 'g1'))));
    expect(find.text('1 person asked to join'), findsOneWidget);
    await tester.pumpWidget(_app(const JoinRequestsPage(conversationId: 'g1')));
    await tester.pumpAndSettle();
    expect(find.text('Cristina Pop'), findsOneWidget);
    await tester.tap(find.text('Approve'));
    await tester.pumpAndSettle();
    expect(calls.any((c) => c.$1 == 'approve_join_request' && c.$2?['account_id'] == 'acc-2'), isTrue);
    expect(find.text('No pending requests.'), findsOneWidget);
  });
  testWidgets('banned: preview says so, admins unban', (tester) async {
    final calls = <(String, Map<String, dynamic>?)>[];
    container = ProviderContainer();
    backend = Backend((cmd, req) async {
      calls.add((cmd, jsonDecode(req) as Map<String, dynamic>?));
      switch (cmd) {
        case 'preview_group_invite':
          return jsonEncode({
            'group_id': 'gid', 'name': 'Book club', 'description': '', 'member_count': 5,
            'approval': false, 'is_member': false, 'requested': false, 'banned': true, 'conversation_id': null,
          });
        case 'group_bans':
          return jsonEncode([
            {'account_id': 'acc-3', 'display_name': 'Dan Rusu', 'avatar_path': null, 'banned_by': 'me', 'banned_at': 1},
          ]);
        case 'unban_group_member':
          return jsonEncode(_group());
      }
      return 'null';
    }, const Stream.empty());
    await tester.pumpWidget(_app(const JoinLinkPage(initial: _link)));
    await tester.pumpAndSettle();
    expect(find.text('You were removed from this group by an admin'), findsOneWidget);
    await tester.tap(find.text('Join group'));
    await tester.pumpAndSettle();
    expect(calls.any((c) => c.$1 == 'join_group_by_link'), isFalse);

    store.upsertGroup(GroupInfo.fromJson(_group()));
    await tester.pumpWidget(_app(const GroupBansPage(conversationId: 'g1')));
    await tester.pumpAndSettle();
    expect(find.text('Dan Rusu'), findsOneWidget);
    await tester.tap(find.text('Unban'));
    await tester.pumpAndSettle();
    expect(calls.any((c) => c.$1 == 'unban_group_member' && c.$2?['account_id'] == 'acc-3'), isTrue);
    expect(find.text('No one is banned from this group.'), findsOneWidget);
  });
}
