// The transcript of an exported conversation: readable text, one entry per message, with the
// date, the time, the sender and what the conversation shows for that message. The core writes
// it out and adds the media (crates/chat-core/src/engine/export.rs).
// Port of app/src/features/chats/exportChat.ts: the two produce the same text.

import '../core/models.dart';
import '../l10n/i18n.dart';
import 'format.dart';

/// An attachment that goes into the archive, under the name the transcript mentions.
class ExportMediaItem {
  final String messageId;
  final String attachmentId;
  final String name;

  const ExportMediaItem({required this.messageId, required this.attachmentId, required this.name});

  Map<String, dynamic> toJson() => {'message_id': messageId, 'attachment_id': attachmentId, 'name': name};
}

class Transcript {
  final String text;
  final List<ExportMediaItem> media;

  /// Messages written (system notes included).
  final int count;

  const Transcript({required this.text, required this.media, required this.count});
}

String _pad(int n, [int width = 2]) => n.toString().padLeft(width, '0');

/// "2026-11-28 14:03" in local time: the same in every language, and it sorts.
String exportTimestamp(int ms) {
  final d = DateTime.fromMillisecondsSinceEpoch(ms);
  return '${d.year}-${_pad(d.month)}-${_pad(d.day)} ${_pad(d.hour)}:${_pad(d.minute)}';
}

const _mimeExt = {'jpeg': 'jpg', 'quicktime': 'mov', 'plain': 'txt', 'mpeg': 'mp3', 'x-wav': 'wav', 'svg+xml': 'svg'};

String _extOfMime(String mime) {
  final parts = mime.split('/');
  final sub = (parts.length > 1 ? parts[1] : '').split(';').first.trim().toLowerCase();
  return _mimeExt[sub] ?? (RegExp(r'^[a-z0-9]{2,5}$').hasMatch(sub) ? sub : 'bin');
}

/// A file name every system accepts: no path, no reserved characters, not hidden, of a sane
/// length (the extension survives the cut).
String safeFileName(String name, [String fallback = 'file']) {
  var s = name
      .replaceAll(RegExp(r'[\\/:*?"<>|\u0000-\u001f\u007f]'), '_')
      .replaceAll(RegExp(r'\s+'), ' ')
      .trim()
      .replaceFirst(RegExp(r'^\.+'), '')
      .replaceFirst(RegExp(r'[. ]+$'), '');
  if (s.isEmpty) s = fallback;
  if (s.length > 100) {
    final dot = s.lastIndexOf('.');
    final ext = dot > 0 && s.length - dot <= 10 ? s.substring(dot) : '';
    s = s.substring(0, 100 - ext.length).trimRight() + ext;
  }
  return s;
}

/// What an attachment is, for its label: photos, videos and voice notes by type, the rest are documents.
MessageKind _attachmentKind(Message m, Attachment a) {
  if (a.mime.startsWith('image/')) return 'image';
  if (a.mime.startsWith('video/')) return 'video';
  if (a.mime.startsWith('audio/') && m.kind == 'voice') return 'voice';
  return 'file';
}

/// Names chosen by other people carry invisible direction marks in the UI; a text file reads better without.
String _plain(String s) => s.replaceAll(RegExp('[\\u2066-\\u2069]'), '');

String _oneLine(String s, [int max = 80]) {
  final flat = s.replaceAll(RegExp(r'\s+'), ' ').trim();
  return flat.length > max ? '${flat.substring(0, max - 1)}…' : flat;
}

/// File name of the export, without extension: "Dialog - Ana Pop".
String exportBaseName(String title) => safeFileName('$kBrandName - $title', kBrandName);

/// Builds the transcript from the messages of a conversation, oldest first.
/// Only what the conversation shows is written: a deleted message is its "deleted" note,
/// view-once media is named but never included, expired messages are skipped.
Transcript buildTranscript(Conversation conv, List<Message> messages, String lang, {required bool withMedia, int? now}) {
  String t(String key, [Map<String, Object?>? args]) => translate(lang, key, args: args);
  final at = now ?? DateTime.now().millisecondsSinceEpoch;
  final media = <ExportMediaItem>[];
  final lines = <String>[];
  var count = 0;

  for (final m in messages) {
    if (m.expiresAt != null && m.expiresAt! <= at) continue;
    final stamp = '[${exportTimestamp(m.sentAt)}]';
    count++;
    if (m.kind == 'system') {
      final text = m.system != null ? systemText(m.system!, lang) : '';
      lines.add('$stamp ${_plain(text.isNotEmpty ? text : t('message.groupEvent'))}');
      continue;
    }
    final sender = m.outgoing ? t('common.you') : (m.senderName.isNotEmpty ? m.senderName : conv.title);
    if (m.deleted) {
      lines.add('$stamp $sender: ${t(m.outgoing ? 'message.deletedByYou' : 'message.deleted')}');
      continue;
    }
    final parts = <String>[];
    final q = m.replyTo;
    if (q != null) {
      // The core names us "You" in quotes.
      final who = q.senderName == 'You' ? t('common.you') : q.senderName;
      parts.add('> $who: ${_oneLine((q.text?.isNotEmpty ?? false) ? q.text! : kindLabel(q.kind, lang))}');
    }
    if (m.forwarded) parts.add('(${t('message.forwarded')})');
    for (final a in m.attachments) {
      final kind = _attachmentKind(m, a);
      if (m.isViewOnce) {
        parts.add('[${t(kind == 'voice' ? 'viewOnce.voice' : kind == 'video' ? 'viewOnce.video' : 'viewOnce.photo')}]');
        continue;
      }
      if (a.downloadState == 'expired' || a.expiredAt != null) {
        parts.add('[${t(kind == 'voice' ? 'retention.voiceExpired' : 'retention.mediaExpired')}]');
        continue;
      }
      final label = kindLabel(kind, lang);
      final named = kind == 'file' && a.fileName.isNotEmpty;
      if (!withMedia) {
        // Documents are known by their name; a photo's file name says nothing.
        parts.add(named ? '[$label: ${a.fileName}]' : '[$label]');
      } else if (a.localPath != null) {
        final name = '${_pad(media.length + 1, 4)}-${safeFileName(a.fileName, '$kind.${_extOfMime(a.mime)}')}';
        media.add(ExportMediaItem(messageId: m.id, attachmentId: a.id, name: name));
        parts.add('[$label: $name]');
      } else {
        parts.add('[${named ? '$label: ${a.fileName}' : label} (${t('chat.export.notIncluded')})]');
      }
    }
    final loc = m.location;
    if (loc != null) {
      final place = [loc.name, loc.address].where((s) => s != null && s.isNotEmpty).join(', ');
      parts.add('[${t('kinds.location')}: ${place.isNotEmpty ? '$place, ' : ''}${loc.lat}, ${loc.lng}]');
    }
    for (final c in m.contacts) {
      parts.add('[${t('kinds.contact')}: ${c.name}, ${formatAccountNumber(c.number)}]');
    }
    if (m.text != null && m.text!.isNotEmpty) parts.add(m.editedAt != null ? '${m.text} (${t('message.edited')})' : m.text!);
    lines.add('$stamp $sender: ${parts.join('\n')}');
  }

  final header = [
    t('chat.export.header', {'app': kBrandName, 'name': conv.title}),
    t('chat.export.exportedOn', {'date': exportTimestamp(at)}),
    t('chat.export.messageCount', {'count': count}),
    '',
  ];
  return Transcript(text: '${[...header, ...lines].join('\n')}\n', media: media, count: count);
}
