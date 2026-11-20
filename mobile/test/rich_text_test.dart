import 'dart:convert';
import 'dart:io';
import 'dart:math';

import 'package:chatapp/core/models.dart';
import 'package:chatapp/util/mentions.dart';
import 'package:chatapp/util/rich_text.dart';
import 'package:flutter_test/flutter_test.dart';

Object? _norm(Seg g) => g.isContainer
    ? {'k': g.kind.name, 'c': g.children.map(_norm).toList()}
    : {'k': g.kind.name, 'text': g.text, 'href': g.href, 'id': g.accountId};

void main() {
  // The cases and expected results come from the desktop parser (app/src/lib/richText.ts),
  // so both apps format messages the same way.
  test('formatting matches the desktop parser', () {
    final cases = jsonDecode(File('test/fixtures/rich_text_cases.json').readAsStringSync()) as List;
    for (final c in cases.cast<Map<String, dynamic>>()) {
      final mentions = (c['mentions'] as List).map((m) => Mention.fromJson(m as Map<String, dynamic>)).toList();
      final segs = parseRich(c['text'] as String, mentions);
      expect(jsonEncode(segs.map(_norm).toList()), jsonEncode(c['segs']), reason: c['text'] as String);
      expect(plainText(c['text'] as String, mentions), c['plain'], reason: c['text'] as String);
    }
  });

  // Adversarial texts of the longest size the core lets through (MAX_TEXT = 65536 characters;
  // a modified sender is cut to that on receipt). Parsing runs on the UI thread for every
  // bubble and chat list preview: it must neither exhaust the stack nor stall.
  group('hostile messages', () {
    const max = 65536;
    String fill(String unit) => List.filled(max ~/ unit.length, unit).join();
    final texts = <String, String>{
      'thousands of bold spans': fill('*a* '),
      'spans without separators': fill('*a*_b_~c~'),
      'unclosed markers': fill('*a '),
      'unclosed markers of every kind': fill('*a _b ~c '),
      'only markers': fill('*_~'),
      'one marker repeated': fill('*'),
      'deep nesting': '${fill('*_~').substring(0, max ~/ 2)}a${fill('~_*').substring(0, max ~/ 2 - 1)}',
      'nesting that never closes': '${fill('*_~')}a',
      'markers split by newlines': fill('*a\n'),
      'unclosed code fences': fill('``` '),
      'unclosed inline code': fill('`a '),
      'only backticks': fill('`'),
      'closed code spans': fill('`a` '),
      'thousands of links': fill('www.a.bc '),
      'links inside spans': fill('*www.a.bc* '),
      'invite-like text': fill('chatapp://join/ '),
      'placeholder characters': fill('*a*'),
      'placeholders with links': 'www.a.bc ${fill('** ')}'.substring(0, max),
      'surrogates and marks': fill('*😀*_́_ '),
      'lone surrogates': fill('*\ud83d* '),
      'whitespace only': fill(' \n\t'),
      'one long word': fill('a'),
    };
    for (final e in texts.entries) {
      test(e.key, () {
        expect(e.value.length, lessThanOrEqualTo(max));
        final sw = Stopwatch()..start();
        final segs = parseRich(e.value);
        final plain = plainText(e.value);
        expect(sw.elapsed, lessThan(const Duration(seconds: 2)), reason: 'parsing took ${sw.elapsedMilliseconds} ms');
        expect(segs, isNotEmpty);
        expect(plain, isNotNull);
      });
    }

    test('every span of a long message is still formatted', () {
      final spans = parseRich(fill('*a* '));
      expect(spans.where((g) => g.kind == SegKind.bold).length, max ~/ 4);
      expect(parseRich(fill('*a ')).every((g) => g.kind == SegKind.text), isTrue);
    });

    test('nested marks keep their structure', () {
      final segs = parseRich('x *_~a~_* y');
      expect(segs.map((g) => g.kind), [SegKind.text, SegKind.bold, SegKind.text]);
      final italic = segs[1].children.single;
      final strike = italic.children.single;
      expect([italic.kind, strike.kind, strike.children.single.text], [SegKind.italic, SegKind.strike, 'a']);
    });

    test('mention ranges from a hostile sender', () {
      final text = fill('*a* ');
      final mentions = [
        const Mention(accountId: 'x', start: -5, length: 3),
        const Mention(accountId: 'x', start: 0, length: -1),
        const Mention(accountId: 'x', start: 0, length: 1 << 40),
        const Mention(accountId: 'x', start: max - 1, length: 5),
        const Mention(accountId: 'x', start: 1 << 40, length: 1),
        for (var i = 0; i < 9000; i++) Mention(accountId: 'x', start: i * 4, length: 3),
        for (var i = 0; i < 2000; i++) Mention(accountId: 'x', start: i, length: 7),
      ];
      final sw = Stopwatch()..start();
      expect(parseRich(text, mentions), isNotEmpty);
      expect(plainText(text, mentions), isNotEmpty);
      expect(sw.elapsed, lessThan(const Duration(seconds: 5)), reason: 'parsing took ${sw.elapsedMilliseconds} ms');
    });

    test('random texts never throw', () {
      const alphabet = ['*', '_', '~', '`', '```', ' ', '\n', 'a', 'é', '😀', '1', '.', 'www.a.bc', 'https://x.y/z_w*', '@', '', '', '!', '(', ')'];
      final rnd = Random(20261120);
      for (var n = 0; n < 3000; n++) {
        final b = StringBuffer();
        final len = rnd.nextInt(n % 100 == 0 ? 4000 : 60);
        for (var i = 0; i < len; i++) {
          b.write(alphabet[rnd.nextInt(alphabet.length)]);
        }
        final text = b.toString();
        final mentions = [
          for (var i = rnd.nextInt(3); i > 0; i--) Mention(accountId: 'x', start: rnd.nextInt(text.length + 3) - 1, length: rnd.nextInt(8) - 1),
        ];
        expect(() => parseRich(text, mentions), returnsNormally, reason: jsonEncode(text));
        expect(() => plainText(text, mentions), returnsNormally, reason: jsonEncode(text));
      }
    });
  });

  test('mention helpers', () {
    expect(mentionQuery('hi @an', 6), (3, 'an'));
    expect(mentionQuery('mail@an', 7), isNull);
    expect(mentionQuery('@ x', 3), isNull);
    expect(filterMentionCandidates(['Ana Maria', 'Bob', 'Ștefan'], (s) => s, 'ma'), ['Ana Maria']);
    expect(filterMentionCandidates(['Ștefan'], (s) => s, 'ste'), ['Ștefan']);
    final picks = [const MentionPick('a', 'Ana'), const MentionPick('b', 'Ana Maria')];
    final ms = collectMentions('😀 @Ana Maria și @Ana, @Anabel', picks);
    expect(ms.map((m) => (m.accountId, m.start, m.length)).toList(), [('b', 3, 10), ('a', 17, 4)]);
    expect(picksFromMentions('x @Bob y', [const Mention(accountId: 'b', start: 2, length: 4)]).single.name, 'Bob');
    expect(wrapSelection('hello world', 6, 11, '*'), ('hello *world*', 7, 12));
    expect(wrapSelection('hello *world*', 7, 12, '*'), ('hello world', 6, 11));
    expect(wrapSelection('hi ', 0, 3, '_'), ('_hi_ ', 1, 3));
  });

  test('punycode hosts for non-ASCII links', () {
    expect(punycodeEncode('münchen'), 'mnchen-3ya');
    expect(punycodeEncode('аpple'), 'pple-43d');
    expect(punycodeEncode('例え'), 'r8jz45g');
    expect(asciiLink('https://аpple.com/login?x=ü'), 'https://xn--pple-43d.com/login?x=ü');
    expect(asciiLink('www.münchen.de'), 'www.xn--mnchen-3ya.de');
    expect(asciiLink('https://example.com/ü'), 'https://example.com/ü');
    final link = parseRich('see https://аpple.com now').firstWhere((g) => g.kind == SegKind.link);
    expect(link.text, 'https://xn--pple-43d.com');
    expect(link.href, 'https://xn--pple-43d.com');
  });
}
