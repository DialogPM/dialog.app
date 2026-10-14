// Formatting helpers (port of app/src/lib/format.ts and messageText.ts).

import 'dart:convert';
import 'package:intl/intl.dart';

import '../core/models.dart';
import '../l10n/i18n.dart';
import 'rich_text.dart';

String formatAccountNumber(String n) =>
    digitsOnly(n).replaceAllMapped(RegExp(r'(\d{4})(?=\d)'), (m) => '${m.group(1)} ');

String digitsOnly(String s) => s.replaceAll(RegExp(r'\D'), '');

String _intlLocale(String lang) => lang == 'en' ? 'en_US' : lang;

String formatBytes(int bytes, String lang) {
  const units = ['B', 'KB', 'MB', 'GB'];
  var v = bytes.toDouble();
  var i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  final nf = NumberFormat.decimalPattern(_intlLocale(lang))..maximumFractionDigits = v < 10 && i > 0 ? 1 : 0;
  return '${nf.format(v)} ${units[i]}';
}

String formatDuration(int ms) {
  final total = (ms / 1000).round().clamp(0, 1 << 31);
  final m = total ~/ 60;
  final s = total % 60;
  return '$m:${s.toString().padLeft(2, '0')}';
}

DateTime _d(int ts) => DateTime.fromMillisecondsSinceEpoch(ts);

int startOfDay(int ts) {
  final d = _d(ts);
  return DateTime(d.year, d.month, d.day).millisecondsSinceEpoch;
}

String dayKey(int ts) {
  final d = _d(ts);
  return '${d.year}-${d.month}-${d.day}';
}

int _daysAgo(int ts) {
  final now = DateTime.now();
  final a = DateTime(now.year, now.month, now.day);
  final d = _d(ts);
  final b = DateTime(d.year, d.month, d.day);
  return (a.difference(b).inHours / 24).round();
}

String formatTime(int ts, String lang) =>
    (lang == 'en' ? DateFormat('hh:mm a', 'en_US') : DateFormat.Hm(_intlLocale(lang))).format(_d(ts));

String formatDateTime(int ts, String lang) {
  final l = _intlLocale(lang);
  return '${DateFormat.yMMMd(l).format(_d(ts))}, ${formatTime(ts, lang)}';
}

/// Chat list: time today, "Yesterday", weekday this week, date otherwise.
String formatListTime(int ts, String lang) {
  final d = _daysAgo(ts);
  if (d <= 0) return formatTime(ts, lang);
  if (d == 1) return translate(lang, 'time.yesterday');
  final l = _intlLocale(lang);
  if (d < 7) return DateFormat.EEEE(l).format(_d(ts));
  return (lang == 'en' ? DateFormat('MM/dd/yyyy') : DateFormat('dd.MM.yyyy')).format(_d(ts));
}

/// Date separator in the message list.
String formatDaySeparator(int ts, String lang) {
  final d = _daysAgo(ts);
  if (d <= 0) return translate(lang, 'time.today');
  if (d == 1) return translate(lang, 'time.yesterday');
  final l = _intlLocale(lang);
  if (d < 7) return DateFormat.EEEE(l).format(_d(ts));
  final sameYear = _d(ts).year == DateTime.now().year;
  return (sameYear ? DateFormat.MMMMd(l) : DateFormat.yMMMMd(l)).format(_d(ts));
}

String formatLastSeen(int ts, String lang) {
  final d = _daysAgo(ts);
  final time = formatTime(ts, lang);
  if (d <= 0) return translate(lang, 'presence.lastSeenToday', args: {'time': time});
  if (d == 1) return translate(lang, 'presence.lastSeenYesterday', args: {'time': time});
  return translate(lang, 'presence.lastSeenDate', args: {'date': DateFormat.MMMd(_intlLocale(lang)).format(_d(ts)), 'time': time});
}

String initials(String name) {
  final clean = name.trim();
  if (clean.isEmpty) return '?';
  if (RegExp(r'^[\d\s+]+$').hasMatch(clean)) return '#';
  final parts = clean.split(RegExp(r'\s+')).where((p) => p.isNotEmpty).toList();
  String first(String s) => s.runes.isEmpty ? '' : String.fromCharCode(s.runes.first);
  final a = first(parts.first);
  final b = parts.length > 1 ? first(parts.last) : '';
  return (a + b).toUpperCase();
}

class TextPart {
  final String text;
  final String? href;
  const TextPart(this.text, [this.href]);
}

final _urlRe = RegExp(r'''\b((?:https?://|www\.)[^\s<>"']+[^\s<>"'.,;:!?)\]])''', caseSensitive: false);

/// Splits text into plain parts and links.
List<TextPart> linkify(String text) {
  final out = <TextPart>[];
  var last = 0;
  for (final m in _urlRe.allMatches(text)) {
    if (m.start > last) out.add(TextPart(text.substring(last, m.start)));
    final raw = m.group(0)!;
    out.add(TextPart(raw, raw.toLowerCase().startsWith('www.') ? 'https://$raw' : raw));
    last = m.end;
  }
  if (last < text.length) out.add(TextPart(text.substring(last)));
  return out;
}

bool _isPictographic(int r) =>
    (r >= 0x1F000 && r <= 0x1FAFF) ||
    (r >= 0x2600 && r <= 0x27BF) ||
    (r >= 0x2300 && r <= 0x23FF) ||
    (r >= 0x2B00 && r <= 0x2BFF) ||
    (r >= 0x2190 && r <= 0x21FF) ||
    (r >= 0x25A0 && r <= 0x25FF) ||
    (r >= 0x2934 && r <= 0x2935) ||
    const {0x00A9, 0x00AE, 0x203C, 0x2049, 0x2122, 0x2139, 0x24C2, 0x3030, 0x303D, 0x3297, 0x3299}.contains(r);

bool _isEmojiJoiner(int r) =>
    r == 0xFE0F || r == 0x200D || r == 0x20E3 || (r >= 0x1F3FB && r <= 0x1F3FF) || (r >= 0xE0020 && r <= 0xE007F);

/// True when the text is only 1-3 emoji (rendered big, like WhatsApp).
bool isEmojiOnly(String text) {
  final t = text.trim();
  if (t.isEmpty || t.length > 24) return false;
  var count = 0;
  var afterJoiner = false;
  var pendingFlag = false;
  for (final r in t.runes) {
    if (r == 0x20 || r == 0x0A) {
      afterJoiner = false;
      pendingFlag = false;
      continue;
    }
    if (r >= 0x1F1E6 && r <= 0x1F1FF) {
      // Two regional indicators make one flag.
      if (!pendingFlag) count++;
      pendingFlag = !pendingFlag;
      continue;
    }
    pendingFlag = false;
    if (_isEmojiJoiner(r)) {
      afterJoiner = r == 0x200D;
      continue;
    }
    if (!_isPictographic(r)) return false;
    if (!afterJoiner) count++;
    afterJoiner = false;
  }
  return count >= 1 && count <= 3;
}

String mapsUrl(double lat, double lng) => 'https://www.openstreetmap.org/?mlat=$lat&mlon=$lng#map=16/$lat/$lng';

/// Minimum length of a new password; same rules as the core (util.rs, password_problems).
const int kMinPassword = 12;
const int kPassphraseLen = 20;
const _commonPasswords = {
  'password', 'parola', 'passwort', 'motdepasse', 'qwerty', 'qwertz', 'azerty', 'asdf', 'letmein', 'welcome', //
  'admin', 'chatapp', 'iloveyou', 'abc', 'abcdef', 'monkey', 'dragon', 'football', 'secret', 'test',
};

/// The rules a new password does not meet yet: "length", "mix", "common" (empty: accepted).
List<String> passwordProblems(String pw) {
  final chars = pw.runes.toList();
  final out = <String>[];
  if (chars.length < kMinPassword) out.add('length');
  final classes = [RegExp(r'\p{Ll}', unicode: true), RegExp(r'\p{Lu}', unicode: true), RegExp('[0-9]'), RegExp(r'[^\p{L}\p{N}]', unicode: true)]
      .where((r) => r.hasMatch(pw))
      .length;
  if (classes < 3 && chars.length < kPassphraseLen) out.add('mix');
  final letters = RegExp(r'\p{L}', unicode: true).allMatches(pw).map((m) => m.group(0)!).join().toLowerCase();
  final sameChar = chars.isNotEmpty && chars.every((c) => c == chars.first);
  final sequence = letters.isEmpty || ('abcdefghijklmnopqrstuvwxyz'.contains(letters) && letters.length < 8);
  if (sameChar || sequence || _commonPasswords.contains(letters)) out.add('common');
  return out;
}

/// Simple password strength estimate: 0 (very weak) .. 4 (strong).
int passwordStrength(String pw) {
  if (pw.isEmpty) return 0;
  var score = 0;
  if (pw.length >= 8) score++;
  if (pw.length >= 12) score++;
  if (pw.length >= 16) score++;
  final classes = [RegExp('[a-z]'), RegExp('[A-Z]'), RegExp(r'\d'), RegExp('[^A-Za-z0-9]')].where((r) => r.hasMatch(pw)).length;
  if (classes >= 3) score++;
  if (RegExp(r'^(.)\1+$').hasMatch(pw) || RegExp(r'^(password|123456|qwerty|parola|letmein)[\d!.]*$', caseSensitive: false).hasMatch(pw)) {
    score = 0;
  }
  if (pw.length < 8) score = score.clamp(0, 1);
  return score.clamp(0, 4);
}

/// Lower-case file extension without the dot ("" if none).
String extOf(String name) {
  final i = name.lastIndexOf('.');
  return i > 0 ? name.substring(i + 1).toLowerCase() : '';
}

// ---------------------------------------------------------------------------
// message texts
// ---------------------------------------------------------------------------

String kindLabel(MessageKind kind, String lang) => switch (kind) {
      'image' => translate(lang, 'kinds.photo'),
      'video' => translate(lang, 'kinds.video'),
      'file' => translate(lang, 'kinds.file'),
      'voice' => translate(lang, 'kinds.voice'),
      'location' => translate(lang, 'kinds.location'),
      'contact' => translate(lang, 'kinds.contact'),
      _ => '',
    };

/// Short text for chat list previews, notifications and quotes. [mediaExpired]: the media was
/// erased by the retention period (a placeholder unless there is a caption).
String previewText(MessageKind kind, String? text, String lang, {bool mediaExpired = false}) {
  if (kind == 'text') return plainText(text ?? '');
  if (text != null && text.isNotEmpty) return plainText(text);
  if (mediaExpired) return translate(lang, kind == 'voice' ? 'retention.voiceExpired' : 'retention.mediaExpired');
  return kindLabel(kind, lang);
}

String _names(List<String> list, String lang) {
  final mapped = list.map((n) => n == 'You' ? translate(lang, 'common.you') : isolate(n)).toList();
  if (mapped.length <= 1) return mapped.join();
  return '${mapped.sublist(0, mapped.length - 1).join(', ')} ${translate(lang, 'common.and')} ${mapped.last}';
}

/// Wraps a name chosen by someone else in a bidi isolate (FSI…PDI), so right-to-left text in
/// it cannot reorder the sentence around it.
String isolate(String name) => '\u2068$name\u2069';

/// Timers offered for disappearing messages, shortest first: (seconds, locale key).
const kDisappearingOptions = [
  (0, 'off'),
  (600, 'm10'),
  (3600, 'h1'),
  (3 * 3600, 'h3'),
  (6 * 3600, 'h6'),
  (12 * 3600, 'h12'),
  (86400, 'h24'),
  (7 * 86400, 'd7'),
  (90 * 86400, 'd90'),
];

String disappearingLabel(int seconds, String lang) {
  if (seconds == 0) return translate(lang, 'disappearing.off');
  // The smallest offered timer that is at least `seconds` (other clients may set any value).
  for (final (s, key) in kDisappearingOptions) {
    if (s > 0 && seconds <= s) return translate(lang, 'disappearing.$key');
  }
  return translate(lang, 'disappearing.d90');
}

String systemText(SystemEvent s, String lang) {
  String t(String key, [Map<String, Object?>? args]) => translate(lang, key, args: args);
  // The event is about us alone ("X added you").
  final onlyYou = s.targetNames.length == 1 && s.targetNames.first == 'You';
  final you = s.actorName == 'You';
  final actor = you ? t('common.you') : s.actorName != null ? isolate(s.actorName!) : t('common.someone');
  final targets = _names(s.targetNames, lang);
  switch (s.type) {
    case 'group_created':
      return t(you ? 'system.groupCreatedYou' : 'system.groupCreated', {'actor': actor, 'name': isolate(s.value ?? '')});
    case 'group_member_added':
      return t(onlyYou ? 'system.memberAddedYou' : 'system.memberAdded', {'actor': actor, 'targets': targets});
    case 'group_member_unverified':
      return t('system.memberUnverified', {'targets': targets});
    case 'group_member_removed':
      return t(onlyYou ? 'system.memberRemovedYou' : 'system.memberRemoved', {'actor': actor, 'targets': targets});
    case 'group_member_joined_link':
      return t(you ? 'system.joinedLinkYou' : 'system.joinedLink', {'actor': actor});
    case 'group_member_left':
      return t(you ? 'system.memberLeftYou' : 'system.memberLeft', {'actor': actor});
    case 'group_renamed':
      return t('system.renamed', {'actor': actor, 'name': isolate(s.value ?? '')});
    case 'group_avatar_changed':
      return t('system.avatarChanged', {'actor': actor});
    case 'group_admin_granted':
      return t('system.adminGranted', {'actor': actor, 'targets': targets});
    case 'group_admin_revoked':
      return t('system.adminRevoked', {'actor': actor, 'targets': targets});
    case 'group_history_cleared':
      return t(you ? 'system.historyClearedYou' : 'system.historyCleared', {'actor': actor});
    case 'disappearing_changed':
      final secs = int.tryParse(s.value ?? '0') ?? 0;
      return secs > 0
          ? t(you ? 'system.disappearingOnYou' : 'system.disappearingOn', {'actor': actor, 'duration': disappearingLabel(secs, lang)})
          : t(you ? 'system.disappearingOffYou' : 'system.disappearingOff', {'actor': actor});
    case 'identity_changed':
      return t('system.identityChanged', {'actor': actor});
    case 'contact_joined':
      return t('system.contactJoined', {'actor': actor});
    case 'call':
      return _callText(s.value, you, lang);
    case 'group_call':
      return _groupCallText(s.value, actor, you, lang);
    case 'identity_mismatch':
      return t('system.identityMismatch', {'actor': actor});
    case 'decryption_failed':
      return t('system.decryptionFailed', {'actor': actor});
    default:
      return '';
  }
}

/// Group call history entry: "Group call · 12:05" (joined) or "Ana started a group call".
String _groupCallText(String? value, String actor, bool you, String lang) {
  String t(String key, [Map<String, Object?>? args]) => translate(lang, key, args: args);
  var duration = 0;
  var joined = false;
  try {
    final v = jsonDecode(value ?? '{}');
    if (v is Map) {
      duration = v['duration_ms'] is num ? (v['duration_ms'] as num).toInt() : 0;
      joined = v['joined'] == true;
    }
  } catch (_) {}
  if (joined) {
    final length = '${duration ~/ 60000}:${((duration % 60000) ~/ 1000).toString().padLeft(2, '0')}';
    return t('groupCall.history.joined', {'duration': length});
  }
  return you ? t('groupCall.history.startedByYou') : t('groupCall.history.missed', {'actor': actor});
}

/// Call history entry: "Outgoing voice call · 1:05", "Missed voice call", ...
String _callText(String? value, bool outgoing, String lang) {
  String t(String key, [Map<String, Object?>? args]) => translate(lang, key, args: args);
  var status = 'missed';
  var duration = 0;
  try {
    final v = jsonDecode(value ?? '{}');
    if (v is Map) {
      status = v['status'] is String ? v['status'] as String : status;
      duration = v['duration_ms'] is num ? (v['duration_ms'] as num).toInt() : 0;
    }
  } catch (_) {}
  if (status == 'answered') {
    final length = '${duration ~/ 60000}:${((duration % 60000) ~/ 1000).toString().padLeft(2, '0')}';
    return t(outgoing ? 'call.history.outgoing' : 'call.history.incoming', {'duration': length});
  }
  if (outgoing) {
    if (status == 'declined') return t('call.history.declinedByPeer');
    if (status == 'busy') return t('call.history.busy');
    if (status == 'failed') return t('call.history.failed');
    // The peer turned calls off (hangup reason "calls_disabled").
    if (status == 'unavailable') return t('call.history.notAccepted');
    return t('call.history.noAnswer');
  }
  if (status == 'declined') return t('call.history.declinedByYou');
  if (status == 'failed') return t('call.history.failed');
  return t('call.history.missed');
}
