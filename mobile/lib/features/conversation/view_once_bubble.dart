// Bubble body of a view-once photo / video / voice message: a placeholder, never a thumbnail
// or an inline player. Incoming unopened: tap to open the viewer (which plays a voice message
// once). Otherwise only the state is shown.

import 'package:flutter/material.dart';

import '../../core/models.dart';
import '../../l10n/i18n.dart';
import '../../theme/app_theme.dart';
import '../../widgets/app_icon.dart';
import '../../widgets/ui.dart';
import 'view_once_viewer.dart';

/// Chat list / bubble label of a view-once message: "View-once photo" / "View-once video" /
/// "Voice message · view once".
String viewOnceLabel(String kind, String lang) => translate(
    lang, kind == 'voice' ? 'viewOnce.voice' : kind == 'video' ? 'mobile.viewOnce.video' : 'mobile.viewOnce.photo');

bool _isVideo(Message m) => m.kind == 'video' || (m.attachments.isNotEmpty && m.attachments.first.isVideo);

/// Signal-style view-once mark: a "1" in a circle (dashed once used up).
class ViewOnceGlyph extends StatelessWidget {
  const ViewOnceGlyph({super.key, this.size = 20, required this.color});
  final double size;
  final Color color;

  @override
  Widget build(BuildContext context) => Container(
        width: size,
        height: size,
        decoration: BoxDecoration(shape: BoxShape.circle, border: Border.all(color: color, width: size / 12)),
        alignment: Alignment.center,
        child: Text('1', style: ts(size * .55, weight: 800, color: color, height: 1)),
      );
}

/// The round "1" toggle shown before sending media or a voice message (accent when on).
class ViewOnceToggle extends StatelessWidget {
  const ViewOnceToggle({super.key, required this.on, required this.onPressed, this.size = 44});
  final bool on;
  final VoidCallback? onPressed;
  final double size;

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    return Tooltip(
      message: context.t('viewOnce.toggle'),
      child: SizedBox(
        width: size,
        height: size,
        child: Tap(
          onTap: onPressed,
          color: on ? t.accent : t.sunk,
          radius: BorderRadius.circular(size / 2),
          semanticLabel: '${context.t('viewOnce.toggle')}: ${context.t(on ? 'mobile.viewOnce.stateOn' : 'mobile.viewOnce.stateOff')}',
          child: Center(child: ViewOnceGlyph(size: 21, color: on ? t.onAccent : t.ink)),
        ),
      ),
    );
  }
}

class ViewOnceBody extends StatelessWidget {
  const ViewOnceBody({super.key, required this.m, required this.fg, required this.meta, required this.accent, this.enabled = true});

  final Message m;
  final Color fg;
  final Color meta;
  final Color accent;

  /// False while selecting messages (a tap selects instead).
  final bool enabled;

  @override
  Widget build(BuildContext context) {
    final voice = m.kind == 'voice';
    final video = !voice && _isVideo(m);
    final state = m.viewOnce;
    final canOpen = !m.outgoing && state == 'unopened';
    final String title;
    String? subtitle;
    if (m.outgoing) {
      title = context.t(voice ? 'viewOnce.voice' : video ? 'mobile.viewOnce.video' : 'mobile.viewOnce.photo');
      if (state == 'opened') subtitle = context.t('mobile.viewOnce.opened');
      if (state == 'expired') subtitle = context.t('mobile.viewOnce.expired');
    } else if (canOpen) {
      title = context.t(voice ? 'viewOnce.voice' : video ? 'mobile.viewOnce.viewVideo' : 'mobile.viewOnce.viewPhoto');
      if (voice) subtitle = context.t('viewOnce.tapToListen');
    } else if (voice) {
      title = context.t('viewOnce.voice');
      subtitle = context.t(state == 'expired' ? 'mobile.viewOnce.expired' : 'mobile.viewOnce.opened');
    } else {
      title = context.t(state == 'expired' ? 'mobile.viewOnce.expired' : 'mobile.viewOnce.opened');
    }
    final active = canOpen;
    final row = Row(mainAxisSize: MainAxisSize.min, children: [
      Container(
        width: 36,
        height: 36,
        decoration: BoxDecoration(
          shape: BoxShape.circle,
          color: active ? accent : Colors.transparent,
          border: active ? null : Border.all(color: meta, width: 1.5),
        ),
        child: Center(
          child: AppIcon(
            active ? (video || voice ? 'play' : 'eye') : (state == 'unopened' ? 'eye' : 'eyeoff'),
            size: 17,
            color: active ? Colors.white : meta,
          ),
        ),
      ),
      const SizedBox(width: 10),
      Flexible(
        child: Column(crossAxisAlignment: CrossAxisAlignment.start, mainAxisSize: MainAxisSize.min, children: [
          Text(
            title,
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            style: ts(14.5, weight: active || m.outgoing ? 650 : 500, color: active || m.outgoing ? fg : meta,
                fontStyle: active || m.outgoing ? null : FontStyle.italic),
          ),
          if (subtitle != null) Text(subtitle, style: ts(12.5, color: meta)),
        ]),
      ),
    ]);
    final body = Padding(padding: const EdgeInsets.fromLTRB(0, 2, 6, 4), child: row);
    if (!canOpen || !enabled) return body;
    return Semantics(
      button: true,
      child: GestureDetector(
        behavior: HitTestBehavior.opaque,
        onTap: () => openViewOnceViewer(context, m),
        child: body,
      ),
    );
  }
}
