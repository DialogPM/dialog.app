// Attachment views inside bubbles: image grid, video, file card; download / save / share.

import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';
import 'dart:ui' show ImageFilter;

import 'package:flutter/material.dart';
import 'package:url_launcher/url_launcher.dart';

import '../../services/media.dart';
import '../../app/globals.dart';
import '../../core/models.dart';
import '../../l10n/i18n.dart';
import '../../services/files.dart';
import '../../state/actions.dart';
import '../../theme/app_theme.dart';
import '../../util/format.dart';
import '../../widgets/app_icon.dart';
import '../../widgets/dialogs.dart';
import '../../widgets/ui.dart';
import 'lightbox.dart';
import 'request_bar.dart' show downloadChecked;

void downloadAttachment(Message m, Attachment a) {
  if (a.downloadState == 'downloading') return;
  downloadChecked(m, a, () => attempt(() => backend.downloadAttachment(m.id, a.id)));
}

/// Saves a decrypted attachment where the user chooses.
Future<void> saveAttachment(Message m, Attachment a) async {
  final path = a.localPath;
  if (path == null) {
    downloadAttachment(m, a);
    return;
  }
  final ok = await attempt(() => saveFileAs(path, a.fileName, a.mime));
  if (ok == true) notifyOk(tr('message.saved'));
}

/// Executables and installers (list of Signal-Desktop isFileDangerous, plus Android packages):
/// opening one from a chat can install or run code.
final _dangerousRe = RegExp(
  r'\.(ADE|ADP|APK|APKS|AAB|XAPK|BAT|CAB|CHM|CMD|COM|CPL|DIAGCAB|DLL|DMG|EXE|HTA|INF|INS|ISP|JAR|JS|JSE|LIB|LNK|MDE|MHT|MSC|MSI|MSP|MST|NSH|PIF|PS1|PSC1|PSM1|PSRC|REG|SCR|SCT|SETTINGCONTENT-MS|SHB|SYS|VB|VBE|VBS|VXD|WSC|WSF|WSH)(\.|\s)*?$',
  caseSensitive: false,
);

bool isFileDangerous(String fileName, [String mime = '']) =>
    _dangerousRe.hasMatch(fileName) || mime == 'application/vnd.android.package-archive';

/// Asks before a dangerous file leaves the app (open / share); true when it may go on.
Future<bool> confirmDangerousFile(Attachment a) async {
  if (!isFileDangerous(a.fileName, a.mime)) return true;
  final ctx = navigatorKey.currentContext;
  if (ctx == null || !ctx.mounted) return false;
  return showConfirm(
    ctx,
    title: tr('mobile.dangerousFileTitle'),
    body: tr('mobile.dangerousFileBody', args: {'name': a.fileName}),
    confirmLabel: tr('mobile.dangerousFileOpen'),
  );
}

Future<void> shareAttachment(Message m, Attachment a) async {
  final path = a.localPath;
  if (path == null) {
    downloadAttachment(m, a);
    return;
  }
  if (!await confirmDangerousFile(a)) return;
  await attempt(() => shareFile(path, a.fileName, a.mime));
}

/// Opens a downloaded file with the system viewer (desktop), or the share sheet (phones).
Future<void> openAttachment(Message m, Attachment a) async {
  final path = a.localPath;
  if (path == null) {
    downloadAttachment(m, a);
    return;
  }
  if (isMobilePlatform) {
    await shareAttachment(m, a);
  } else {
    if (!await confirmDangerousFile(a)) return;
    // The system viewer needs a real file: a temporary decrypted copy (removed at next start).
    await attempt(() async => launchUrl(Uri.file((await exportMedia(path, a.fileName, inUse: false)).path)));
  }
}

/// Decodes a `data:` URL thumbnail.
Uint8List? thumbBytes(String? dataUrl) {
  if (dataUrl == null) return null;
  final i = dataUrl.indexOf('base64,');
  if (i < 0) return null;
  try {
    return base64Decode(dataUrl.substring(i + 7));
  } catch (_) {
    return null;
  }
}

class _DownloadBadge extends StatelessWidget {
  const _DownloadBadge({required this.a, required this.onTap});
  final Attachment a;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    if (a.downloadState == 'downloading') {
      final p = a.progress ?? 0;
      return SizedBox.square(
        dimension: 48,
        child: Stack(alignment: Alignment.center, children: [
          CircularProgressIndicator(value: p > 0 ? p : null, strokeWidth: 3.5, color: Colors.white, backgroundColor: Colors.white24),
          if (p > 0) Text('${(p * 100).round()}%', style: ts(11, color: Colors.white)),
        ]),
      );
    }
    return GestureDetector(
      onTap: onTap,
      child: Container(
        width: 48,
        height: 48,
        decoration: const BoxDecoration(color: Color.fromRGBO(0, 0, 0, .45), shape: BoxShape.circle),
        child: const Center(child: AppIcon('download', size: 22, color: Colors.white)),
      ),
    );
  }
}

Widget _placeholder(BuildContext context, Attachment a) {
  final bytes = thumbBytes(a.thumbnail);
  if (bytes != null) {
    return ImageFiltered(
      imageFilter: ImageFilterBlur.sigma(8),
      child: SafeMemoryImage(bytes, fit: BoxFit.cover, cacheWidth: 400),
    );
  }
  return const ImgPlaceholder();
}

/// Striped placeholder of the design (--img-ph).
class ImgPlaceholder extends StatelessWidget {
  const ImgPlaceholder({super.key, this.label});
  final String? label;

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    return CustomPaint(
      painter: _StripePainter(t.sunk, t.line),
      child: label == null ? const SizedBox.expand() : Center(child: Text(label!, style: ts(11, color: t.faint, mono: true))),
    );
  }
}

class _StripePainter extends CustomPainter {
  const _StripePainter(this.bg, this.line);
  final Color bg;
  final Color line;

  @override
  void paint(Canvas canvas, Size size) {
    canvas.drawRect(Offset.zero & size, Paint()..color = bg);
    final p = Paint()
      ..color = line
      ..strokeWidth = 1;
    canvas.save();
    canvas.clipRect(Offset.zero & size);
    for (double d = -size.height; d < size.width + size.height; d += 12 * 1.4142) {
      canvas.drawLine(Offset(d, 0), Offset(d + size.height, size.height), p);
    }
    canvas.restore();
  }

  @override
  bool shouldRepaint(_StripePainter old) => old.bg != bg || old.line != line;
}

/// One or more images (up to 4 tiles, "+N" on the last one).
class ImageGrid extends StatelessWidget {
  const ImageGrid({super.key, required this.message, required this.attachments});
  final Message message;
  final List<Attachment> attachments;

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    final shown = attachments.take(4).toList();
    final extra = attachments.length - shown.length;
    final radius = BorderRadius.circular((t.rBubble - 5).clamp(4, 40));
    Widget tile(int i, Attachment a) {
      final path = a.localPath;
      final has = path != null && File(path).existsSync();
      return GestureDetector(
        onTap: () => has ? openLightbox(context, message, i) : downloadAttachment(message, a),
        child: Stack(fit: StackFit.expand, children: [
          if (has) MediaImage(path, fit: BoxFit.cover, cacheWidth: 900) else _placeholder(context, a),
          if (!has)
            Container(
              color: Colors.black12,
              child: Column(mainAxisAlignment: MainAxisAlignment.center, children: [
                _DownloadBadge(a: a, onTap: () => downloadAttachment(message, a)),
                if (a.downloadState != 'downloading') ...[
                  const SizedBox(height: 6),
                  Text(formatBytes(a.size, context.lang), style: ts(12, weight: 600, color: Colors.white, shadows: const [Shadow(blurRadius: 4)])),
                ],
              ]),
            ),
          if (extra > 0 && i == shown.length - 1 && has)
            Container(color: Colors.black45, alignment: Alignment.center, child: Text('+$extra', style: ts(24, weight: 700, color: Colors.white))),
        ]),
      );
    }

    if (shown.length == 1) {
      final a = shown.first;
      final ratio = (a.width != null && a.height != null && a.height! > 0) ? (a.width! / a.height!).clamp(0.6, 1.9) : 1.5;
      return ClipRRect(
        borderRadius: radius,
        child: SizedBox(width: 300, child: AspectRatio(aspectRatio: ratio, child: tile(0, a))),
      );
    }
    return ClipRRect(
      borderRadius: radius,
      child: SizedBox(
        width: 300,
        child: GridView.count(
          crossAxisCount: 2,
          shrinkWrap: true,
          physics: const NeverScrollableScrollPhysics(),
          mainAxisSpacing: 3,
          crossAxisSpacing: 3,
          padding: EdgeInsets.zero,
          children: [for (var i = 0; i < shown.length; i++) tile(i, shown[i])],
        ),
      ),
    );
  }
}

class VideoAttachment extends StatelessWidget {
  const VideoAttachment({super.key, required this.message, required this.a});
  final Message message;
  final Attachment a;

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    final has = a.localPath != null;
    return ClipRRect(
      borderRadius: BorderRadius.circular((t.rBubble - 5).clamp(4, 40)),
      child: SizedBox(
        width: 300,
        child: AspectRatio(
          aspectRatio: 16 / 10,
          child: GestureDetector(
            onTap: () => has ? openAttachment(message, a) : downloadAttachment(message, a),
            child: Stack(fit: StackFit.expand, children: [
              _placeholder(context, a),
              Container(
                color: Colors.black26,
                child: Column(mainAxisAlignment: MainAxisAlignment.center, children: [
                  if (a.downloadState == 'downloading')
                    _DownloadBadge(a: a, onTap: () {})
                  else
                    Container(
                      width: 52,
                      height: 52,
                      decoration: const BoxDecoration(color: Color.fromRGBO(0, 0, 0, .45), shape: BoxShape.circle),
                      child: Center(child: AppIcon(has ? 'play' : 'download', size: has ? 28 : 22, color: Colors.white)),
                    ),
                  const SizedBox(height: 6),
                  Text(
                    '${a.durationMs != null ? '${formatDuration(a.durationMs!)} · ' : ''}${formatBytes(a.size, context.lang)}',
                    style: ts(12, weight: 600, color: Colors.white, shadows: const [Shadow(blurRadius: 4)]),
                  ),
                ]),
              ),
            ]),
          ),
        ),
      ),
    );
  }
}

class FileIcon extends StatelessWidget {
  const FileIcon({super.key, required this.name, this.bg});
  final String name;
  final Color? bg;

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    final ext = extOf(name);
    return Container(
      width: 42,
      height: 48,
      decoration: BoxDecoration(color: bg ?? t.sunk, borderRadius: BorderRadius.circular(9)),
      alignment: Alignment.center,
      child: ext.isEmpty
          ? AppIcon('file', size: 20, color: t.muted)
          : Text(ext.length > 4 ? ext.substring(0, 4).toUpperCase() : ext.toUpperCase(), style: ts(10, weight: 600, mono: true, color: t.ink)),
    );
  }
}

class FileCard extends StatelessWidget {
  const FileCard({super.key, required this.message, required this.a, required this.metaColor, this.iconBg});
  final Message message;
  final Attachment a;
  final Color metaColor;
  final Color? iconBg;

  @override
  Widget build(BuildContext context) {
    final lang = context.lang;
    final ext = extOf(a.fileName).toUpperCase();
    final downloaded = a.localPath != null;
    final progress = a.downloadState == 'downloading' && a.progress != null ? ' · ${(a.progress! * 100).round()}%' : '';
    return GestureDetector(
      onTap: () => downloaded ? openAttachment(message, a) : downloadAttachment(message, a),
      child: Container(
        width: 250,
        padding: const EdgeInsets.symmetric(horizontal: 2, vertical: 4),
        child: Row(children: [
          FileIcon(name: a.fileName, bg: iconBg),
          const SizedBox(width: 12),
          Expanded(
            child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
              Text(a.fileName, maxLines: 1, overflow: TextOverflow.ellipsis, style: ts(14.5, weight: 650)),
              Text('${formatBytes(a.size, lang)}${ext.isNotEmpty ? ' · $ext' : ''}$progress', style: ts(12, color: metaColor)),
            ]),
          ),
          const SizedBox(width: 6),
          if (a.downloadState == 'downloading')
            SizedBox.square(dimension: 20, child: CircularProgressIndicator(strokeWidth: 2, color: metaColor))
          else
            IconBtn(
              downloaded ? 'share' : 'download',
              size: 32,
              iconSize: 17,
              round: true,
              color: metaColor,
              tooltip: context.t(downloaded ? 'mobile.share' : 'message.download'),
              onPressed: () => downloaded ? shareAttachment(message, a) : downloadAttachment(message, a),
            ),
        ]),
      ),
    );
  }
}

/// Media erased by the retention period (engine/retention.rs): a neutral placeholder with no
/// download, save or share action. Documents keep their name.
class ExpiredMediaCard extends StatelessWidget {
  const ExpiredMediaCard({super.key, required this.attachments, required this.voice, required this.metaColor});
  final List<Attachment> attachments;
  final bool voice;
  final Color metaColor;

  @override
  Widget build(BuildContext context) {
    final names = attachments.map((a) => a.fileName).where((n) => n.isNotEmpty).join(', ');
    return Container(
      width: 250,
      padding: const EdgeInsets.symmetric(horizontal: 4, vertical: 6),
      child: Row(children: [
        AppIcon(voice ? 'micoff' : 'image', size: 22, color: metaColor),
        const SizedBox(width: 12),
        Expanded(
          child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
            Text(context.t(voice ? 'retention.voiceExpired' : 'retention.mediaExpired'),
                style: ts(14, color: metaColor, fontStyle: FontStyle.italic)),
            if (names.isNotEmpty) Text(names, maxLines: 1, overflow: TextOverflow.ellipsis, style: ts(12, color: metaColor)),
          ]),
        ),
      ]),
    );
  }
}

/// Blur filter helper.
abstract final class ImageFilterBlur {
  static ImageFilter sigma(double s) => ImageFilter.blur(sigmaX: s, sigmaY: s);
}
