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
      final suffix = RegExp(r'_(zero|one|two|few|many|other)$');
      final en = kStrings['en']!.keys.where((k) => !suffix.hasMatch(k)).toSet();
      expect(kStrings.keys.toSet(), {for (final l in kLanguages) l.code});
      for (final l in kLanguages) {
        final keys = kStrings[l.code]!.keys.map((k) => k.replaceAll(suffix, '')).toSet();
        final missing = en.where((k) => !keys.contains(k)).toList();
        expect(missing, isEmpty, reason: '${l.code} is missing $missing');
      }
    });

    test('every plural string has the form each language needs', () {
      final suffix = RegExp(r'_(zero|one|two|few|many|other)$');
      final plurals = kStrings['en']!.keys.where(suffix.hasMatch).map((k) => k.replaceAll(suffix, '')).toSet();
      for (final l in kLanguages) {
        for (final key in plurals) {
          for (final n in [0, 1, 2, 3, 5, 11, 21, 22, 25, 100, 101, 111]) {
            final cat = pluralCategory(l.code, n);
            expect(kStrings[l.code]!.containsKey('${key}_$cat') || kStrings[l.code]!.containsKey('${key}_other'), isTrue,
                reason: '${l.code}: ${key}_$cat');
            expect(translate(l.code, key, count: n), isNot(contains('{{count}}')), reason: '${l.code}: $key ($n)');
          }
        }
      }
    });

    test('plural categories', () {
      expect([1, 2, 5, 11, 21, 22, 25].map((n) => pluralCategory('ru', n)), ['one', 'few', 'many', 'many', 'one', 'few', 'many']);
      expect([1, 2, 5, 12, 22].map((n) => pluralCategory('pl', n)), ['one', 'few', 'many', 'many', 'few']);
      expect([0, 1, 2, 3, 11, 100].map((n) => pluralCategory('ar', n)), ['zero', 'one', 'two', 'few', 'many', 'other']);
      expect([0, 1, 2].map((n) => pluralCategory('fr', n)), ['one', 'one', 'other']);
      expect([1, 2].map((n) => pluralCategory('ja', n)), ['other', 'other']);
      expect([0, 1, 2].map((n) => pluralCategory('de', n)), ['other', 'one', 'other']);
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
