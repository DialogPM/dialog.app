import 'dart:convert';

import 'package:chatapp/app/globals.dart';
import 'package:chatapp/core/backend.dart';
import 'package:chatapp/core/models.dart';
import 'package:chatapp/features/onboarding/profile_picker.dart';
import 'package:chatapp/l10n/i18n.dart';
import 'package:chatapp/state/profiles.dart';
import 'package:chatapp/theme/app_theme.dart';
import 'package:chatapp/theme/tokens.dart';
import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:flutter_test/flutter_test.dart';

Widget _app(Widget child) => UncontrolledProviderScope(
      container: container,
      child: MaterialApp(
        theme: buildTheme(ChatTokens.build(Direction.a, dark: false)),
        home: L10n(lang: 'en', child: Scaffold(body: SingleChildScrollView(child: child))),
      ),
    );

DeviceInfo _device(int id, {bool current = false}) =>
    DeviceInfo(deviceId: id, name: 'd$id', platform: 'ios', createdAt: 0, lastSeenAt: 0, current: current);

const _ana = ProfileCard(id: 'p1', label: 'Ana', last4: '5678', serverHost: 'chat.example.com', lastUsed: 2);
const _hiddenish = ProfileCard(id: 'default');

void main() {
  group('profile helpers', () {
    test('cards from the registry listing', () {
      final list = (jsonDecode('[{"id":"p1","label":"Ana","last4":"5678","server_host":"chat.example.com","last_used":2},'
              '{"id":"default","label":null,"last4":null,"server_host":null,"last_used":0}]') as List)
          .map((e) => ProfileCard.fromJson(Map<String, dynamic>.from(e as Map)))
          .toList();
      expect(list.map((p) => p.id), ['p1', 'default']);
      expect(list.first.serverHost, 'chat.example.com');
      expect(list.last.label, isNull);
      expect(CurrentProfile.fromJson({'registered': true, 'visible': false}).visible, isFalse);
      expect(CurrentProfile.fromJson({'registered': false}).visible, isTrue);
    });

    test('name and subtitle', () {
      expect(profileName(_ana, 'Account'), 'Ana');
      expect(profileSubtitle(_ana), '•••• 5678 · chat.example.com');
      // The account of an older version is listed before it is opened once: no details yet.
      expect(profileName(_hiddenish, 'Account'), 'Account');
      expect(profileSubtitle(_hiddenish), '');
      expect(profileName(const ProfileCard(id: 'x', label: '  '), 'Account'), 'Account');
    });

    test('only one profile opens its password field right away', () {
      expect(initiallyOpenProfile([_ana]), 'p1');
      expect(initiallyOpenProfile([_ana, _hiddenish]), isNull);
      expect(initiallyOpenProfile(const []), isNull);
    });

    test('last device of the account', () {
      expect(isOnlyDevice([_device(1, current: true)]), isTrue);
      expect(isOnlyDevice(const []), isTrue);
      expect(isOnlyDevice([_device(1, current: true), _device(2)]), isFalse);
    });
  });

  group('start screen', () {
    testWidgets('a profile opens with its password; a wrong one keeps the start screen', (tester) async {
      final calls = <(String, Object?)>[];
      container = ProviderContainer();
      backend = Backend((cmd, req) async {
        calls.add((cmd, jsonDecode(req)));
        switch (cmd) {
          case 'profile_open':
            if ((jsonDecode(req) as Map)['password'] != 'right one') throw 'invalid_password: wrong password';
            return 'null';
          case 'get_state':
            return jsonEncode({'status': 'no_account', 'default_server_url': ''});
        }
        return 'null';
      }, const Stream.empty());
      await tester.pumpWidget(_app(ProfileList(profiles: const [_ana, _hiddenish], onChanged: () {})));
      expect(find.text('Ana'), findsOneWidget);
      expect(find.text('•••• 5678 · chat.example.com'), findsOneWidget);
      expect(find.text('Account'), findsOneWidget);
      // Two profiles: nothing is open until one is tapped.
      expect(find.byType(TextField), findsNothing);
      await tester.tap(find.text('Ana'));
      await tester.pumpAndSettle();
      expect(find.byType(TextField), findsOneWidget);

      await tester.enterText(find.byType(TextField), 'nope');
      await tester.pump();
      await tester.tap(find.text('Open'));
      await tester.pumpAndSettle();
      expect(calls.map((c) => c.$1), ['profile_open']);
      expect(find.textContaining('Wrong password'), findsOneWidget);

      await tester.enterText(find.byType(TextField), 'right one');
      await tester.pump();
      await tester.tap(find.text('Open'));
      // Opened: the app state is rebuilt (the start screen goes away in the real app).
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 100));
      expect(calls.map((c) => c.$1), ['profile_open', 'profile_open', 'get_state']);
      expect(calls[1].$2, {'id': 'p1', 'password': 'right one'});
      expect(appData.appState?.status, 'no_account');
    });

    testWidgets('another account by its full number', (tester) async {
      final calls = <(String, Object?)>[];
      container = ProviderContainer();
      backend = Backend((cmd, req) async {
        calls.add((cmd, jsonDecode(req)));
        if (cmd == 'profile_open_by_number') throw 'invalid_password: wrong account number or password';
        return 'null';
      }, const Stream.empty());
      await tester.pumpWidget(_app(OpenOtherProfile(onBack: () {})));
      final fields = find.byType(TextField);
      await tester.enterText(fields.at(0), '1234 5678 1234 5678');
      await tester.enterText(fields.at(1), 'secret');
      await tester.pumpAndSettle();
      await tester.tap(find.text('Open'));
      await tester.pumpAndSettle();
      expect(calls.single.$1, 'profile_open_by_number');
      expect(calls.single.$2, {'number': '1234567812345678', 'password': 'secret'});
      expect(find.text('Wrong account number or password.'), findsOneWidget);
    });
  });
}
