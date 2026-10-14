// Message text rendering model: WhatsApp-style formatting (*bold*, _italic_, ~strike~,
// `code`, ```code block```), @mentions (ranges from the message) and links.
// Formatting is display-only: the text travels as typed. Same rules as the desktop app
// (app/src/lib/richText.ts); offsets are UTF-16 code units in both.

import '../core/models.dart';

enum SegKind { text, link, mention, code, pre, bold, italic, strike }

class Seg {
  final SegKind kind;
  final String text;
  final String? href;
  final String? accountId;
  final List<Seg> children;

  const Seg(this.kind, {this.text = '', this.href, this.accountId, this.children = const []});

  bool get isContainer => kind == SegKind.bold || kind == SegKind.italic || kind == SegKind.strike;
}

// Mentions and links are replaced by private-use characters before parsing, so formatting
// markers inside them (e.g. "_" in a URL) are left alone.
const _ph = 0xe000;
const _maxTokens = 6000;

final _urlRe = RegExp(r'''\b((?:https?://|www\.)[^\s<>"']+[^\s<>"'.,;:!?)\]])''', caseSensitive: false);
final _inviteRe = RegExp(r'chatapp://join/[A-Za-z0-9_-]{22}(?![A-Za-z0-9_-])');
final _wordRe = RegExp(r'[\p{L}\p{N}]', unicode: true);
final _spaceRe = RegExp(r'\s');
final _codeRe = RegExp(r'```([\s\S]+?)```|`([^`\n]+)`');

const _marks = {'*': SegKind.bold, '_': SegKind.italic, '~': SegKind.strike};

class _Token {
  final Seg seg;
  final String raw;
  const _Token(this.seg, this.raw);
}

class _Range {
  final int start;
  final int end;
  final Seg seg;
  const _Range(this.start, this.end, this.seg);
}

String? _at(String s, int i) => i >= 0 && i < s.length ? s[i] : null;
bool _isWord(String? c) => c != null && _wordRe.hasMatch(c);
bool _isSpace(String? c) => c != null && _spaceRe.hasMatch(c);

(String, List<_Token>) _protect(String text, List<Mention> mentions) {
  final ranges = <_Range>[];
  final sorted = [...mentions]..sort((a, b) => a.start.compareTo(b.start));
  for (final m in sorted) {
    final end = m.start + m.length;
    if (m.length <= 0 || m.start < 0 || end > text.length || ranges.any((r) => r.end > m.start)) continue;
    ranges.add(_Range(m.start, end, Seg(SegKind.mention, text: text.substring(m.start, end), accountId: m.accountId)));
  }
  // Group invite links (chatapp://join/...) open the join page.
  for (final m in _inviteRe.allMatches(text)) {
    if (ranges.any((r) => m.start < r.end && m.end > r.start)) continue;
    ranges.add(_Range(m.start, m.end, Seg(SegKind.link, text: m.group(0)!, href: m.group(0))));
  }
  for (final m in _urlRe.allMatches(text)) {
    if (ranges.any((r) => m.start < r.end && m.end > r.start)) continue;
    final raw = m.group(0)!;
    // Homograph links ("аpple.com" with a Cyrillic "а"): the host is shown and opened in
    // punycode (xn--...) as soon as it has a non-ASCII character.
    final shown = asciiLink(raw);
    final href = RegExp(r'^https?:', caseSensitive: false).hasMatch(shown) ? shown : 'https://$shown';
    ranges.add(_Range(m.start, m.end, Seg(SegKind.link, text: shown, href: href)));
  }
  ranges.sort((a, b) => a.start.compareTo(b.start));
  final tokens = <_Token>[];
  final out = StringBuffer();
  var last = 0;
  for (final r in ranges.take(_maxTokens)) {
    out
      ..write(text.substring(last, r.start))
      ..writeCharCode(_ph + tokens.length);
    tokens.add(_Token(r.seg, text.substring(r.start, r.end)));
    last = r.end;
  }
  out.write(text.substring(last));
  return (out.toString(), tokens);
}

_Token? _tokenAt(String s, int i, List<_Token> tokens) {
  final c = s.codeUnitAt(i) - _ph;
  return c >= 0 && c < tokens.length ? tokens[c] : null;
}

/// Restores the original characters of protected tokens (inside code).
String _restore(String s, List<_Token> tokens) {
  final out = StringBuffer();
  for (var i = 0; i < s.length; i++) {
    final tok = _tokenAt(s, i, tokens);
    if (tok != null) {
      out.write(tok.raw);
    } else {
      out.writeCharCode(s.codeUnitAt(i));
    }
  }
  return out.toString();
}

List<Seg> _leaves(String s, List<_Token> tokens) {
  final out = <Seg>[];
  var from = 0;
  for (var i = 0; i < s.length; i++) {
    final tok = _tokenAt(s, i, tokens);
    if (tok == null) continue;
    if (i > from) out.add(Seg(SegKind.text, text: s.substring(from, i)));
    out.add(tok.seg);
    from = i + 1;
  }
  if (from < s.length) out.add(Seg(SegKind.text, text: s.substring(from)));
  return out;
}

/// Inline formatting of one piece without code spans.
List<Seg> _inline(String s, List<_Token> tokens, int depth) {
  if (depth > 4) return _leaves(s, tokens);
  for (var i = 0; i < s.length; i++) {
    final kind = _marks[s[i]];
    if (kind == null) continue;
    // Opening marker: at the start or after a non-word character, followed by a non-space.
    if (_isWord(_at(s, i - 1)) || _isSpace(_at(s, i + 1)) || i + 1 >= s.length || s[i + 1] == s[i]) continue;
    for (var j = i + 2; j < s.length; j++) {
      if (s[j] == '\n') break;
      // Closing marker: after a non-space, followed by the end or a non-word character.
      if (s[j] == s[i] && !_isSpace(_at(s, j - 1)) && !_isWord(_at(s, j + 1))) {
        return [
          ..._leaves(s.substring(0, i), tokens),
          Seg(kind, children: _inline(s.substring(i + 1, j), tokens, depth + 1)),
          ..._inline(s.substring(j + 1), tokens, depth),
        ];
      }
    }
  }
  return _leaves(s, tokens);
}

/// Parses a message text into display segments.
List<Seg> parseRich(String text, [List<Mention> mentions = const []]) {
  final (s, tokens) = _protect(text, mentions);
  final out = <Seg>[];
  // Code first: ```blocks``` (may span lines), then `inline`.
  var last = 0;
  for (final m in _codeRe.allMatches(s)) {
    out.addAll(_inline(s.substring(last, m.start), tokens, 0));
    if (m.group(1) != null) {
      out.add(Seg(SegKind.pre, text: _restore(m.group(1)!.replaceAll(RegExp(r'^\n|\n$'), ''), tokens)));
    } else {
      out.add(Seg(SegKind.code, text: _restore(m.group(2)!, tokens)));
    }
    last = m.end;
  }
  out.addAll(_inline(s.substring(last), tokens, 0));
  return out;
}

/// Text without formatting markers (previews, notifications, copying).
String plainText(String text, [List<Mention> mentions = const []]) {
  String flat(List<Seg> segs) => segs.map((g) => g.isContainer ? flat(g.children) : g.text).join();
  return flat(parseRich(text, mentions));
}

// ---------------------------------------------------------------------------
// Punycode (RFC 3492) for link hosts
// ---------------------------------------------------------------------------

final _hostRe = RegExp(r'^((?:https?://)?(?:[^@/?#]*@)?)([^/?#:]+)(.*)$', caseSensitive: false, dotAll: true);

/// [link] with every non-ASCII label of its host converted to punycode ("xn--...").
String asciiLink(String link) {
  final m = _hostRe.firstMatch(link);
  if (m == null) return link;
  final host = m.group(2)!;
  if (!host.runes.any((r) => r > 0x7f)) return link;
  final labels = host.split(RegExp('[.。．｡]')).map((l) {
    if (!l.runes.any((r) => r > 0x7f)) return l;
    final enc = punycodeEncode(l.toLowerCase());
    return enc == null ? l : 'xn--$enc';
  });
  return '${m.group(1)}${labels.join('.')}${m.group(3)}';
}

/// Punycode encoding of one label (without the "xn--" prefix); null on overflow.
String? punycodeEncode(String input) {
  const base = 36, tMin = 1, tMax = 26, skew = 38, damp = 700, initialBias = 72, initialN = 128;
  final cps = input.runes.toList();
  final out = StringBuffer();
  for (final c in cps) {
    if (c < 0x80) out.writeCharCode(c);
  }
  final b = out.length;
  var h = b;
  if (b > 0) out.write('-');
  String digit(int d) => String.fromCharCode(d < 26 ? 97 + d : 22 + d);
  int adapt(int delta, int numPoints, bool first) {
    delta = first ? delta ~/ damp : delta ~/ 2;
    delta += delta ~/ numPoints;
    var k = 0;
    while (delta > ((base - tMin) * tMax) ~/ 2) {
      delta ~/= base - tMin;
      k += base;
    }
    return k + ((base - tMin + 1) * delta) ~/ (delta + skew);
  }

  var n = initialN, delta = 0, bias = initialBias;
  while (h < cps.length) {
    final m = cps.where((c) => c >= n).reduce((a, c) => c < a ? c : a);
    if ((m - n) > (0x7fffffff - delta) ~/ (h + 1)) return null;
    delta += (m - n) * (h + 1);
    n = m;
    for (final c in cps) {
      if (c < n && ++delta > 0x7fffffff) return null;
      if (c == n) {
        var q = delta;
        for (var k = base;; k += base) {
          final t = k <= bias ? tMin : (k >= bias + tMax ? tMax : k - bias);
          if (q < t) break;
          out.write(digit(t + (q - t) % (base - t)));
          q = (q - t) ~/ (base - t);
        }
        out.write(digit(q));
        bias = adapt(delta, h + 1, h == b);
        delta = 0;
        h++;
      }
    }
    delta++;
    n++;
  }
  return out.toString();
}
