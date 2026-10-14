// "Send to …" screen for picked photos / videos / documents, with a caption.

import 'dart:io';
import 'dart:ui' as ui;

import 'package:flutter/material.dart';

import '../../app/globals.dart';
import '../../core/models.dart';
import '../../l10n/i18n.dart';
import '../../services/files.dart';
import '../../state/actions.dart';
import '../../theme/app_theme.dart';
import '../../util/format.dart';
import '../../widgets/app_icon.dart';
import '../../widgets/toast.dart';
import '../../widgets/ui.dart';
import 'attachments.dart';
import 'view_once_bubble.dart' show ViewOnceToggle;

void openSendMedia(BuildContext context, String conversationId, List<PickedFile> files, {required bool media}) {
  Navigator.of(context).push(MaterialPageRoute<void>(builder: (_) => SendMediaPage(conversationId: conversationId, files: files, media: media)));
}

Future<({int width, int height})?> imageSize(String path) async {
  try {
    final buffer = await ui.ImmutableBuffer.fromFilePath(path);
    final desc = await ui.ImageDescriptor.encoded(buffer);
    final r = (width: desc.width, height: desc.height);
    desc.dispose();
    buffer.dispose();
    return r;
  } catch (_) {
    return null;
  }
}

class SendMediaPage extends StatefulWidget {
  const SendMediaPage({super.key, required this.conversationId, required this.files, required this.media});
  final String conversationId;
  final List<PickedFile> files;
  final bool media;

  @override
  State<SendMediaPage> createState() => _SendMediaPageState();
}

class _SendMediaPageState extends State<SendMediaPage> {
  late final List<PickedFile> _files = [...widget.files];
  // Every file shown here: their temporary copies are removed when the page closes (sent or not).
  late final List<PickedFile> _picked = [...widget.files];
  final _caption = TextEditingController();
  bool _sending = false;
  bool _viewOnceOn = false;

  /// View once is possible only for exactly one photo or video.
  bool get _canViewOnce => widget.media && _files.length == 1 && (_files.first.isImage || _files.first.isVideo);
  bool get _viewOnce => _viewOnceOn && _canViewOnce;

  void _toggleViewOnce() {
    setState(() {
      _viewOnceOn = !_viewOnce;
      // A view-once message carries no caption.
      if (_viewOnceOn) _caption.clear();
    });
    showToast(tr(_viewOnceOn ? 'mobile.viewOnce.on' : 'mobile.viewOnce.off'));
  }

  @override
  void dispose() {
    _caption.dispose();
    if (!_sending) discardPicked(_picked);
    super.dispose();
  }

  Future<void> _send() async {
    final max = appData.serverInfo?.maxAttachmentBytes ?? 0;
    final tooBig = max > 0 ? _files.where((f) => (f.size ?? 0) > max).toList() : const <PickedFile>[];
    if (tooBig.isNotEmpty) {
      showToast(tr('composer.tooBig', args: {'name': tooBig.first.name, 'max': formatBytes(max, currentLanguage)}), error: true);
      return;
    }
    setState(() => _sending = true);
    try {
      if (_viewOnce) {
        final f = _files.first;
        final size = f.isImage ? await imageSize(f.path) : null;
        await sendMessage(
          conversationId: widget.conversationId,
          attachments: [
            OutgoingAttachment(
              path: f.path,
              fileName: f.name,
              mime: f.mime,
              kind: f.isImage ? 'image' : 'video',
              width: size?.width,
              height: size?.height,
            ),
          ],
          viewOnce: true,
        );
        if (mounted) Navigator.of(context).pop();
        return;
      }
      final text = _caption.text.trim().isEmpty ? null : _caption.text.trim();
      final images = widget.media ? _files.where((f) => f.isImage).toList() : <PickedFile>[];
      final videos = widget.media ? _files.where((f) => f.isVideo).toList() : <PickedFile>[];
      final others = _files.where((f) => !images.contains(f) && !videos.contains(f)).toList();
      var captionUsed = false;
      if (images.isNotEmpty) {
        final atts = <OutgoingAttachment>[];
        for (final f in images) {
          final size = await imageSize(f.path);
          atts.add(OutgoingAttachment(path: f.path, fileName: f.name, mime: f.mime, kind: 'image', width: size?.width, height: size?.height));
        }
        await sendMessage(conversationId: widget.conversationId, text: text, attachments: atts);
        captionUsed = true;
      }
      for (final f in videos) {
        await sendMessage(
          conversationId: widget.conversationId,
          text: captionUsed ? null : text,
          attachments: [OutgoingAttachment(path: f.path, fileName: f.name, mime: f.mime, kind: 'video')],
        );
        captionUsed = true;
      }
      for (final f in others) {
        await sendMessage(
          conversationId: widget.conversationId,
          text: captionUsed ? null : text,
          attachments: [OutgoingAttachment(path: f.path, fileName: f.name, mime: f.mime, kind: 'file')],
        );
        captionUsed = true;
      }
      if (mounted) Navigator.of(context).pop();
    } finally {
      if (mounted) {
        setState(() => _sending = false);
      } else {
        // Closed while sending: the copies were kept until the core had read them.
        discardPicked(_picked);
      }
    }
  }

  Future<void> _addMore() async {
    final more = await pickFiles(kind: widget.media ? 'media' : 'any', multiple: true);
    if (more.isNotEmpty) {
      _picked.addAll(more);
      setState(() => _files.addAll(more));
    }
  }

  void _remove(PickedFile f) {
    setState(() => _files.remove(f));
    if (_files.isEmpty) Navigator.of(context).pop();
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    final title = appData.conversations[widget.conversationId]?.title ?? '';
    final lang = context.lang;
    Widget removeBtn(PickedFile f) => Positioned(
          top: 6,
          right: 6,
          child: GestureDetector(
            onTap: () => _remove(f),
            child: Container(
              width: 26,
              height: 26,
              decoration: BoxDecoration(color: t.ink, shape: BoxShape.circle),
              child: Center(child: AppIcon('x', size: 14, color: t.pane)),
            ),
          ),
        );
    Widget preview(PickedFile f, {double? height}) => ClipRRect(
          borderRadius: BorderRadius.circular(12),
          child: f.isImage
              ? Image.file(File(f.path), height: height, fit: height == null ? BoxFit.cover : BoxFit.contain, cacheWidth: 1200)
              : Container(
                  color: t.sunk,
                  child: Column(mainAxisAlignment: MainAxisAlignment.center, children: [
                    AppIcon('movie', size: 36, color: t.muted),
                    const SizedBox(height: 4),
                    Padding(
                      padding: const EdgeInsets.symmetric(horizontal: 8),
                      child: Text(f.name, maxLines: 1, overflow: TextOverflow.ellipsis, style: ts(12, color: t.muted)),
                    ),
                  ]),
                ),
        );
    Widget body;
    if (widget.media && _files.length == 1) {
      body = Padding(
        padding: const EdgeInsets.all(16),
        child: Stack(children: [Center(child: preview(_files.first, height: double.infinity)), removeBtn(_files.first)]),
      );
    } else if (widget.media) {
      body = GridView.count(
        crossAxisCount: 2,
        padding: const EdgeInsets.all(16),
        mainAxisSpacing: 8,
        crossAxisSpacing: 8,
        children: [for (final f in _files) Stack(fit: StackFit.expand, children: [preview(f), removeBtn(f)])],
      );
    } else {
      body = ListView(padding: const EdgeInsets.all(16), children: [
        for (final f in _files)
          Container(
            margin: const EdgeInsets.only(bottom: 8),
            padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 8),
            decoration: BoxDecoration(color: t.sunk, borderRadius: BorderRadius.circular(12)),
            child: Row(children: [
              FileIcon(name: f.name, bg: t.pane),
              const SizedBox(width: 12),
              Expanded(
                child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
                  Text(f.name, maxLines: 1, overflow: TextOverflow.ellipsis, style: ts(14, weight: 600, color: t.ink)),
                  Text(f.size != null ? formatBytes(f.size!, lang) : f.mime, style: ts(12, color: t.muted)),
                ]),
              ),
              IconBtn('x', size: 32, iconSize: 16, color: t.muted, tooltip: context.t('common.remove'), onPressed: () => _remove(f)),
            ]),
          ),
      ]);
    }
    return PaneScaffold(
      header: PaneHeader(title: context.t('composer.sendTo', args: {'name': title}), backIcon: 'x'),
      body: body,
      bottom: Container(
        padding: EdgeInsets.fromLTRB(10, 10, 10, 12 + MediaQuery.of(context).padding.bottom),
        decoration: BoxDecoration(color: t.pane, border: Border(top: BorderSide(color: t.line))),
        child: Row(crossAxisAlignment: CrossAxisAlignment.end, children: [
          IconBtn('plus', size: 44, iconSize: 20, round: true, bg: t.sunk, color: t.ink, tooltip: context.t('composer.addMore'), onPressed: _addMore),
          const SizedBox(width: 8),
          Expanded(
            child: Container(
              constraints: const BoxConstraints(minHeight: 44),
              padding: const EdgeInsets.symmetric(horizontal: 16),
              decoration: BoxDecoration(color: t.sunk, borderRadius: BorderRadius.circular(t.rInput.clamp(0, 22))),
              child: TextField(
                enableIMEPersonalizedLearning: imePersonalizedLearning,
                controller: _caption,
                enabled: !_viewOnce,
                minLines: 1,
                maxLines: 4,
                style: ts(15, color: t.ink),
                decoration: InputDecoration(
                  isDense: true,
                  border: InputBorder.none,
                  hintText: context.t(_viewOnce ? 'viewOnce.noCaption' : 'composer.caption'),
                  hintStyle: ts(15, color: t.ink.withValues(alpha: .5)),
                  contentPadding: const EdgeInsets.symmetric(vertical: 12),
                ),
              ),
            ),
          ),
          if (_canViewOnce) ...[
            const SizedBox(width: 8),
            ViewOnceToggle(on: _viewOnce, onPressed: _sending ? null : _toggleViewOnce),
          ],
          const SizedBox(width: 8),
          Btn(
            label: _files.length > 1 ? '${context.t('composer.send')} (${_files.length})' : context.t('composer.send'),
            icon: 'send',
            height: 44,
            expand: false,
            busy: _sending,
            onPressed: _send,
          ),
        ]),
      ),
    );
  }
}
