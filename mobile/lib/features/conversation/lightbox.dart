// Full-screen image viewer: swipe between the images of a message, pinch / double-tap zoom,
// save and share.


import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../services/media.dart';
import '../../core/models.dart';
import '../../l10n/i18n.dart';
import '../../state/store.dart';
import '../../theme/app_theme.dart';
import '../../util/format.dart';
import '../../widgets/ui.dart';
import 'attachments.dart';

void openLightbox(BuildContext context, Message message, int index) {
  Navigator.of(context).push(PageRouteBuilder<void>(
    opaque: false,
    barrierColor: Colors.black,
    pageBuilder: (_, _, _) => Lightbox(messageId: message.id, initial: message, index: index),
    transitionsBuilder: (_, anim, _, child) => FadeTransition(opacity: anim, child: child),
  ));
}

class Lightbox extends ConsumerStatefulWidget {
  const Lightbox({super.key, required this.messageId, required this.initial, required this.index});
  final String messageId;
  final Message initial;
  final int index;

  @override
  ConsumerState<Lightbox> createState() => _LightboxState();
}

class _LightboxState extends ConsumerState<Lightbox> {
  late final PageController _pages = PageController(initialPage: widget.index);
  late int _index = widget.index;
  final _zoom = TransformationController();

  @override
  void dispose() {
    _pages.dispose();
    _zoom.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    // Keep the latest version of the message (download states etc.).
    final message = ref.watch(storeProvider.select((s) => s.messages[widget.messageId])) ?? widget.initial;
    final images = message.attachments.where((a) => a.isImage && a.localPath != null).toList();
    if (images.isEmpty) return const Scaffold(backgroundColor: Colors.black);
    final current = images[_index.clamp(0, images.length - 1)];
    final lang = context.lang;
    final t = context.tk;
    return Scaffold(
      backgroundColor: Colors.black,
      body: SafeArea(
        child: Column(children: [
          Padding(
            padding: const EdgeInsets.fromLTRB(6, 8, 8, 8),
            child: Row(children: [
              IconBtn('back', color: Colors.white, iconSize: 21, tooltip: context.t('common.close'), onPressed: () => Navigator.of(context).pop()),
              const SizedBox(width: 6),
              Avatar(name: message.senderName, seed: message.senderAccountId, size: 36),
              const SizedBox(width: 10),
              Expanded(
                child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
                  Text(message.outgoing ? context.t('common.you') : message.senderName,
                      maxLines: 1, overflow: TextOverflow.ellipsis, style: ts(15, weight: 650, color: Colors.white)),
                  Text('${formatDateTime(message.sentAt, lang)}${images.length > 1 ? ' · ${_index + 1}/${images.length}' : ''}',
                      style: ts(12, color: Colors.white60)),
                ]),
              ),
              IconBtn('share', color: Colors.white, tooltip: context.t('mobile.share'), onPressed: () => shareAttachment(message, current)),
              IconBtn('download', color: Colors.white, tooltip: context.t('message.save'), onPressed: () => saveAttachment(message, current)),
            ]),
          ),
          Expanded(
            child: PageView.builder(
              controller: _pages,
              itemCount: images.length,
              onPageChanged: (i) => setState(() {
                _index = i;
                _zoom.value = Matrix4.identity();
              }),
              itemBuilder: (_, i) => GestureDetector(
                onDoubleTap: () => setState(() {
                  _zoom.value = _zoom.value.getMaxScaleOnAxis() > 1 ? Matrix4.identity() : Matrix4.diagonal3Values(2.5, 2.5, 1);
                }),
                child: InteractiveViewer(
                  transformationController: i == _index ? _zoom : null,
                  minScale: 1,
                  maxScale: 5,
                  child: Center(child: MediaImage(images[i].localPath!, fit: BoxFit.contain)),
                ),
              ),
            ),
          ),
          if (message.text?.isNotEmpty ?? false)
            Container(
              width: double.infinity,
              padding: const EdgeInsets.fromLTRB(20, 12, 20, 16),
              child: Text(message.text!, textAlign: TextAlign.center, style: ts(14.5, color: Colors.white, height: 1.4)),
            ),
          if (images.length > 1)
            Padding(
              padding: const EdgeInsets.only(bottom: 10),
              child: Row(mainAxisAlignment: MainAxisAlignment.center, children: [
                for (var i = 0; i < images.length; i++)
                  Container(
                    width: 7,
                    height: 7,
                    margin: const EdgeInsets.symmetric(horizontal: 3),
                    decoration: BoxDecoration(shape: BoxShape.circle, color: i == _index ? t.accent : Colors.white30),
                  ),
              ]),
            ),
          const SizedBox(height: 4),
        ]),
      ),
    );
  }
}
