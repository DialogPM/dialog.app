import 'dart:convert';
import 'dart:io';

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
