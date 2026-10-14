import 'package:chatapp/l10n/i18n.dart';
import 'package:chatapp/l10n/strings.g.dart';
import 'package:chatapp/theme/oklch.dart';
import 'package:chatapp/theme/tokens.dart';
import 'package:chatapp/util/format.dart';
import 'package:chatapp/widgets/app_icon.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  group('theme tokens', () {
    test('direction A light / dark values from the design', () {
      final a = ChatTokens.build(Direction.a, dark: false);
      expect(a.canvas, const Color(0xFFEEEAE2));
      expect(a.accent, const Color(0xFF0F7A5E));
      expect(a.bubbleOut, const Color(0xFFD8EBDF));
      expect(a.bubbleOutGradient, isNull);
      expect(a.rBubble, 14);
      expect(a.rInput, 12);
      expect(a.font, 'PlusJakartaSans');
      expect(a.wallDot, isNotNull);
      expect(a.sectSep, 8);
      final ad = ChatTokens.build(Direction.a, dark: true);
      expect(ad.pane, const Color(0xFF181715));
      expect(ad.accentText, const Color(0xFF62D4AA));
    });

    test('direction B uses gradients, pills and Figtree', () {
      final b = ChatTokens.build(Direction.b, dark: false);
      expect(b.font, 'Figtree');
      expect(b.rBubble, 20);
      expect(b.rInput, 999);
      expect(b.bubbleOutGradient, isA<LinearGradient>());
      expect((b.bubbleOutGradient! as LinearGradient).colors, [const Color(0xFF0FA283), const Color(0xFF0B8597)]);
      expect(b.wallDot, isNull);
      expect(b.inputRadius(44), BorderRadius.circular(22));
      final bd = ChatTokens.build(Direction.b, dark: true);
      expect(bd.canvas, const Color(0xFF05080A));
      expect(bd.playFgOut, const Color(0xFF0B8597));
    });

    test('oklch avatar colors and hue', () {
      expect(hueFor('maria'), hueFor('maria'));
      expect(hueFor('abc'), inInclusiveRange(0, 359));
      // oklch(0.91 0.045 200) is a light, slightly cyan tint.
      final c = avatarColors(200, dark: false).bg;
      expect(c.g, greaterThan(.9));
      expect(c.b, greaterThan(c.r));
      final white = oklch(1, 0, 0);
      expect(white.r, closeTo(1, .001));
    });
  });

  group('i18n', () {
    test('all languages have the same keys as English', () {
      final en = kStrings['en']!.keys.where((k) => !RegExp(r'_(one|few|other)$').hasMatch(k)).toSet();
      for (final lang in ['ro', 'it']) {
        final keys = kStrings[lang]!.keys.map((k) => k.replaceAll(RegExp(r'_(one|few|other)$'), '')).toSet();
        final missing = en.where((k) => !keys.contains(k)).toList();
        expect(missing, isEmpty, reason: '$lang is missing $missing');
      }
    });

    test('interpolation and Romanian plurals', () {
      expect(translate('en', 'group.members', count: 1), '1 member');
      expect(translate('en', 'group.members', count: 5), '5 members');
      expect(pluralCategory('ro', 1), 'one');
      expect(pluralCategory('ro', 3), 'few');
      expect(pluralCategory('ro', 25), 'other');
      expect(translate('ro', 'group.members', count: 25), contains('25'));
      expect(translate('en', 'chat.delete.title', args: {'name': 'Ana'}), 'Delete chat with Ana?');
      expect(translate('it', 'missing.key'), 'missing.key');
    });
  });

  group('format', () {
    test('account numbers, initials, bytes, duration', () {
      expect(formatAccountNumber('1297346865452344'), '1297 3468 6545 2344');
      expect(initials('Maria Popescu'), 'MP');
      expect(initials('+40 721'), '#');
      expect(formatBytes(1536, 'en'), '1.5 KB');
      expect(formatDuration(9000), '0:09');
      expect(passwordStrength('short'), lessThanOrEqualTo(1));
      expect(passwordStrength('Correct-Horse-9-battery'), 4);
    });

    test('emoji-only detection and links', () {
      expect(isEmojiOnly('👍'), isTrue);
      expect(isEmojiOnly('❤️😂🙏'), isTrue);
      expect(isEmojiOnly('👍🏽'), isTrue);
      expect(isEmojiOnly('🇷🇴'), isTrue);
      expect(isEmojiOnly('hi 👍'), isFalse);
      expect(isEmojiOnly('👍👍👍👍'), isFalse);
      final parts = linkify('see https://example.com/a, ok');
      expect(parts.where((p) => p.href != null).single.href, 'https://example.com/a');
    });
  });

  testWidgets('AppIcon paints every icon', (tester) async {
    await tester.pumpWidget(MaterialApp(
      home: Wrap(children: [for (final n in kIconPaths.keys) AppIcon(n, size: 24, color: Colors.black)]),
    ));
    expect(find.byType(AppIcon), findsNWidgets(kIconPaths.length));
    expect(tester.takeException(), isNull);
  });
}
