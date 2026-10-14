// @mention helpers for the composer (same rules as app/src/lib/mentions.ts): finding the
// "@query" being typed, and turning the members picked from the suggestions into ranges
// (UTF-16, "@" included) when sending.

import '../core/models.dart';

class MentionPick {
  final String accountId;
  final String name;
  const MentionPick(this.accountId, this.name);
}

const maxMentionSuggestions = 8;

final _queryRe = RegExp(r'(^|\s)@([\p{L}\p{N}_.\- ]{0,30})$', unicode: true);
final _wordRe = RegExp(r'[\p{L}\p{N}_]', unicode: true);
final _marksRe = RegExp(r'\p{M}', unicode: true);

/// The "@query" right before the caret, if the user is typing one: (start of "@", query).
(int, String)? mentionQuery(String text, int caret) {
  if (caret < 0 || caret > text.length) return null;
  final m = _queryRe.firstMatch(text.substring(0, caret));
  if (m == null) return null;
  final query = m.group(2)!;
  // A space is allowed only inside a name being typed ("@Ana M"), never twice or at the start.
  if (query.startsWith(' ') || query.contains('  ')) return null;
  return (caret - query.length - 1, query);
}

String _fold(String s) => _stripDiacritics(s).toLowerCase();

const _diacritics = {
  'ă': 'a',
  'Ă': 'A',
  'â': 'a',
  'Â': 'A',
  'á': 'a',
  'Á': 'A',
  'à': 'a',
  'À': 'A',
  'ä': 'a',
  'Ä': 'A',
  'ã': 'a',
  'Ã': 'A',
  'å': 'a',
  'Å': 'A',
  'ā': 'a',
  'Ā': 'A',
  'î': 'i',
  'Î': 'I',
  'í': 'i',
  'Í': 'I',
  'ì': 'i',
  'Ì': 'I',
  'ï': 'i',
  'Ï': 'I',
  'ī': 'i',
  'Ī': 'I',
  'ș': 's',
  'Ș': 'S',
  'ş': 's',
  'Ş': 'S',
  'ś': 's',
  'Ś': 'S',
  'š': 's',
  'Š': 'S',
  'ț': 't',
  'Ț': 'T',
  'ţ': 't',
  'Ţ': 'T',
  'ť': 't',
  'Ť': 'T',
  'é': 'e',
  'É': 'E',
  'è': 'e',
  'È': 'E',
  'ë': 'e',
  'Ë': 'E',
  'ê': 'e',
  'Ê': 'E',
  'ē': 'e',
  'Ē': 'E',
  'ę': 'e',
  'Ę': 'E',
  'ó': 'o',
  'Ó': 'O',
  'ò': 'o',
  'Ò': 'O',
  'ö': 'o',
  'Ö': 'O',
  'ô': 'o',
  'Ô': 'O',
  'õ': 'o',
  'Õ': 'O',
  'ő': 'o',
  'Ő': 'O',
  'ø': 'o',
  'Ø': 'O',
  'ú': 'u',
  'Ú': 'U',
  'ù': 'u',
  'Ù': 'U',
  'ü': 'u',
  'Ü': 'U',
  'û': 'u',
  'Û': 'U',
  'ů': 'u',
  'Ů': 'U',
  'ű': 'u',
  'Ű': 'U',
  'ç': 'c',
  'Ç': 'C',
  'ć': 'c',
  'Ć': 'C',
  'č': 'c',
  'Č': 'C',
  'ñ': 'n',
  'Ñ': 'N',
  'ń': 'n',
  'Ń': 'N',
  'ý': 'y',
  'Ý': 'Y',
  'ÿ': 'y',
  'Ÿ': 'Y',
  'ź': 'z',
  'Ź': 'Z',
  'ż': 'z',
  'Ż': 'Z',
  'ž': 'z',
  'Ž': 'Z',
  'ł': 'l',
  'Ł': 'L',
  'ř': 'r',
  'Ř': 'R',
  'ď': 'd',
  'Ď': 'D',
  'ğ': 'g',
  'Ğ': 'G',
};

String _stripDiacritics(String s) {
  final out = StringBuffer();
  for (final r in s.runes) {
    final c = String.fromCharCode(r);
    out.write(_diacritics[c] ?? c);
  }
  return out.toString().replaceAll(_marksRe, '');
}

/// Candidates whose name (or any word of it) starts with the query.
List<T> filterMentionCandidates<T>(List<T> list, String Function(T) name, String query) {
  final q = _fold(query.trim());
  return list
      .where((c) {
        final n = _fold(name(c));
        return q.isEmpty || n.startsWith(q) || n.split(RegExp(r'\s+')).any((w) => w.startsWith(q));
      })
      .take(maxMentionSuggestions)
      .toList();
}

/// The mention ranges for [text]: every "@Name" of a picked member that still stands as a
/// whole word. Longer names win where names overlap ("@Ana Maria" over "@Ana").
List<Mention> collectMentions(String text, List<MentionPick> picks) {
  final out = <Mention>[];
  final seen = <String>{};
  final sorted = [...picks]..sort((a, b) => b.name.length.compareTo(a.name.length));
  for (final p in sorted) {
    if (p.name.isEmpty || !seen.add('${p.accountId}\u0000${p.name}')) continue;
    final needle = '@${p.name}';
    var from = 0;
    while (true) {
      final i = text.indexOf(needle, from);
      if (i < 0) break;
      from = i + needle.length;
      final end = i + needle.length;
      if (i > 0 && _wordRe.hasMatch(text[i - 1])) continue;
      if (end < text.length && _wordRe.hasMatch(text[end])) continue;
      if (out.any((m) => i < m.start + m.length && end > m.start)) continue;
      out.add(Mention(accountId: p.accountId, start: i, length: needle.length));
    }
  }
  out.sort((a, b) => a.start.compareTo(b.start));
  return out;
}

/// The picks behind existing mentions (editing a message keeps its mentions).
List<MentionPick> picksFromMentions(String text, List<Mention> mentions) => [
  for (final m in mentions)
    if (m.start + m.length <= text.length && m.length > 1) MentionPick(m.accountId, text.substring(m.start + 1, m.start + m.length)),
];

/// Wraps the selection with a formatting marker (or unwraps it when already wrapped).
(String, int, int) wrapSelection(String text, int start, int end, String mark) {
  if (start == end) {
    return (text.replaceRange(start, end, '$mark$mark'), start + mark.length, start + mark.length);
  }
  final sel = text.substring(start, end);
  if (sel.length > 2 * mark.length && sel.startsWith(mark) && sel.endsWith(mark)) {
    final inner = sel.substring(mark.length, sel.length - mark.length);
    return (text.replaceRange(start, end, inner), start, start + inner.length);
  }
  if (start >= mark.length &&
      end + mark.length <= text.length &&
      text.substring(start - mark.length, start) == mark &&
      text.substring(end, end + mark.length) == mark) {
    return (text.replaceRange(start - mark.length, end + mark.length, sel), start - mark.length, end - mark.length);
  }
  // Markers must touch the text: keep surrounding spaces outside.
  final lead = sel.length - sel.trimLeft().length;
  final trail = sel.length - sel.trimRight().length;
  final s = start + lead;
  final e = end - trail;
  final next = text.substring(0, s) + mark + text.substring(s, e) + mark + text.substring(e);
  return (next, s + mark.length, e + mark.length);
}
