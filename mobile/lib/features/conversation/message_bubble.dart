// One message row: bubble with sender, forwarded tag, quote, attachments, text, meta, reactions.
// Long press opens the action sheet (quick reactions + reply/copy/forward/edit/...).

import 'dart:async';

import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:url_launcher/url_launcher.dart';

import '../../app/globals.dart';
import '../../core/models.dart';
import '../../l10n/i18n.dart';
import '../../services/media.dart';
import '../../state/calls.dart' show startCall;
import '../../state/group_calls.dart' show startGroupCall;
import '../../state/actions.dart';
import '../../state/store.dart';
import '../../theme/app_theme.dart';
import '../../theme/oklch.dart';
import '../../theme/tokens.dart';
import '../../util/format.dart';
import '../../util/rich_text.dart';
import '../../widgets/app_icon.dart';
import '../../widgets/dialogs.dart';
import '../../widgets/emoji_data.dart';
import '../../widgets/emoji_picker.dart';
import '../../widgets/ui.dart';
import '../chats/chat_list.dart';
import '../groups/invite_pages.dart';
import 'attachments.dart';
import 'location.dart';
import 'message_dialogs.dart';
import 'message_list.dart';
import 'share_contacts.dart';
import 'view_once_bubble.dart';
import 'voice.dart';

/// Colors of a bubble depending on its direction.
class BubbleColors {
  BubbleColors(ChatTokens t, bool out)
      : bg = out ? t.bubbleOut : t.bubbleIn,
        gradient = out ? t.bubbleOutGradient : null,
        fg = out ? t.bubbleOutFg : t.bubbleInFg,
        meta = out ? t.outMeta : t.inMeta,
        replyBg = out ? t.replyOut : t.replyIn,
        replyBar = out ? t.replyBarOut : t.accentText,
        link = out ? t.linkOut : t.accentText,
        playBg = out ? t.playBgOut : t.accent,
        playFg = out ? t.playFgOut : t.onAccent,
        waveOn = out ? t.waveOnOut : t.accent,
        waveOff = out ? t.waveOffOut : t.faint,
        tickRead = out ? t.tick : t.accentText;

  final Color bg, fg, meta, replyBg, replyBar, link, playBg, playFg, waveOn, waveOff, tickRead;
  final Gradient? gradient;
}

/// Quoted message inside a bubble (or above the composer).
class QuoteBlock extends StatelessWidget {
  const QuoteBlock({super.key, required this.q, this.onTap, required this.bg, required this.bar, this.margin = const EdgeInsets.only(bottom: 7)});

  final QuotedMessage q;
  final VoidCallback? onTap;
  final Color bg;
  final Color bar;
  final EdgeInsetsGeometry margin;

  @override
  Widget build(BuildContext context) {
    final lang = context.lang;
    final you = q.senderName == 'You';
    final text = (q.text?.isNotEmpty ?? false) ? q.text! : kindLabel(q.kind, lang);
    final thumb = thumbBytes(q.thumbnail);
    return GestureDetector(
      onTap: onTap,
      child: Container(
        margin: margin,
        padding: const EdgeInsets.fromLTRB(10, 7, 10, 7),
        decoration: BoxDecoration(
          color: bg,
          borderRadius: BorderRadius.circular(9),
          border: Border(left: BorderSide(color: bar, width: 3)),
        ),
        child: Row(children: [
          Expanded(
            child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
              Text(you ? context.t('common.you') : q.senderName, maxLines: 1, overflow: TextOverflow.ellipsis, style: ts(12.5, weight: 700, color: bar)),
              Opacity(opacity: .82, child: Text(text, maxLines: 2, overflow: TextOverflow.ellipsis, style: ts(13, height: 1.35))),
            ]),
          ),
          if (thumb != null) ...[
            const SizedBox(width: 8),
            ClipRRect(borderRadius: BorderRadius.circular(6), child: SafeMemoryImage(thumb, width: 40, height: 40, fit: BoxFit.cover, cacheWidth: 120)),
          ],
        ]),
      ),
    );
  }
}

class _Meta extends StatelessWidget {
  const _Meta({required this.m, required this.c, this.overlay = false});
  final Message m;
  final BubbleColors c;
  final bool overlay;

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    final failed = m.outgoing && m.status == 'failed';
    final color = overlay ? Colors.white : c.meta;
    final row = Row(mainAxisSize: MainAxisSize.min, children: [
      if (m.starred) ...[AppIcon('star', size: 12, color: color, semanticLabel: context.t('star.starred')), const SizedBox(width: 4)],
      if (m.pinnedUntil != null) ...[AppIcon('pin', size: 12, color: color, semanticLabel: context.t('pin.pinned')), const SizedBox(width: 4)],
      if (m.expiresAt != null) ...[AppIcon('clock', size: 12, color: color), const SizedBox(width: 4)],
      if (m.editedAt != null && !m.deleted) ...[Text(context.t('message.edited'), style: ts(11.5, color: color)), const SizedBox(width: 4)],
      Text(formatTime(m.sentAt, context.lang), style: ts(11.5, color: color)),
      if (m.outgoing && !m.deleted && !failed) ...[
        const SizedBox(width: 4),
        StatusTicks(status: m.status, size: 15, color: color, readColor: overlay ? Colors.white : c.tickRead),
      ],
      if (failed) ...[const SizedBox(width: 4), Text('· ${context.t('status.failed')}', style: ts(11.5, weight: 600, color: t.danger))],
    ]);
    if (!overlay) return row;
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 3),
      decoration: BoxDecoration(color: const Color.fromRGBO(0, 0, 0, .45), borderRadius: BorderRadius.circular(999)),
      child: row,
    );
  }
}

class _RichText extends StatefulWidget {
  const _RichText({required this.text, required this.style, required this.link, this.mentions = const [], required this.codeBg});
  final String text;
  final TextStyle style;
  final Color link;
  final List<Mention> mentions;
  final Color codeBg;

  @override
  State<_RichText> createState() => _RichTextState();
}

/// Message text with formatting, @mentions and links (see util/rich_text.dart).
class _RichTextState extends State<_RichText> {
  final _recognizers = <TapGestureRecognizer>[];

  @override
  void dispose() {
    for (final r in _recognizers) {
      r.dispose();
    }
    super.dispose();
  }

  List<InlineSpan> _spans(List<Seg> segs, String? me) => [
        for (final g in segs)
          switch (g.kind) {
            SegKind.text => TextSpan(text: g.text),
            SegKind.link => TextSpan(
                text: g.text,
                style: TextStyle(color: widget.link, decoration: TextDecoration.underline, decorationColor: widget.link),
                recognizer: (TapGestureRecognizer()
                      ..onTap = () => g.href!.startsWith('chatapp://join/')
                          ? openJoinLink(context, g.href!)
                          : launchUrl(Uri.parse(g.href!), mode: LaunchMode.externalApplication))
                    .also(_recognizers.add),
              ),
            SegKind.mention => TextSpan(
                text: g.text,
                style: TextStyle(
                  color: widget.link,
                  fontWeight: FontWeight.w600,
                  fontVariations: const [FontVariation.weight(600)],
                  backgroundColor: g.accountId == me ? widget.link.withValues(alpha: .14) : null,
                ),
              ),
            SegKind.code || SegKind.pre => TextSpan(
                text: g.text,
                style: TextStyle(fontFamily: kMonoFont, fontSize: (widget.style.fontSize ?? 15) - 1.5, backgroundColor: widget.codeBg),
              ),
            SegKind.bold => TextSpan(
                style: const TextStyle(fontWeight: FontWeight.w700, fontVariations: [FontVariation.weight(700)]),
                children: _spans(g.children, me),
              ),
            SegKind.italic => TextSpan(style: const TextStyle(fontStyle: FontStyle.italic), children: _spans(g.children, me)),
            SegKind.strike => TextSpan(
                style: TextStyle(decoration: TextDecoration.lineThrough, decorationColor: widget.style.color),
                children: _spans(g.children, me),
              ),
          },
      ];

  @override
  Widget build(BuildContext context) {
    for (final r in _recognizers) {
      r.dispose();
    }
    _recognizers.clear();
    final segs = parseRich(widget.text, widget.mentions);
    if (segs.length == 1 && segs.first.kind == SegKind.text) return Text(widget.text, style: widget.style);
    final me = appData.appState?.account?.accountId;
    return Text.rich(TextSpan(children: _spans(segs, me)), style: widget.style);
  }
}

extension _Also<T> on T {
  T also(void Function(T) f) {
    f(this);
    return this;
  }
}

class MessageRow extends ConsumerWidget {
  const MessageRow({
    super.key,
    required this.id,
    required this.first,
    required this.group,
    required this.selecting,
    required this.selected,
    required this.highlighted,
  });

  final String id;

  /// First message of a run from the same sender (larger gap, shows the sender name).
  final bool first;
  final bool group;
  final bool selecting;
  final bool selected;
  final bool highlighted;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final m = ref.watch(storeProvider.select((s) => s.messages[id]));
    if (m == null) return const SizedBox.shrink();
    final t = context.tk;
    if (m.kind == 'system' && m.system?.type == 'group_call') {
      // Group call history: tap to call the group.
      return Padding(
        padding: const EdgeInsets.only(top: 10, bottom: 6),
        child: GestureDetector(
          behavior: HitTestBehavior.opaque,
          onTap: () => unawaited(startGroupCall(m.conversationId)),
          child: SysNote(icon: 'users', text: systemText(m.system!, context.lang)),
        ),
      );
    }
    if (m.kind == 'system' && m.system?.type == 'call') {
      // Call history: tap to call back.
      return Padding(
        padding: const EdgeInsets.only(top: 10, bottom: 6),
        child: GestureDetector(
          behavior: HitTestBehavior.opaque,
          onTap: () => unawaited(startCall(m.conversationId)),
          child: SysNote(icon: 'phone', text: systemText(m.system!, context.lang)),
        ),
      );
    }
    if (m.kind == 'system' && m.system != null) {
      return Padding(
        padding: const EdgeInsets.only(top: 10, bottom: 6),
        child: SysNote(icon: m.system!.type == 'disappearing_changed' ? 'clock' : 'lock', text: systemText(m.system!, context.lang)),
      );
    }
    final out = m.outgoing;
    final c = BubbleColors(t, out);
    // View-once media is never shown inline (view_once_bubble.dart).
    final viewOnce = m.isViewOnce && !m.deleted;
    // Media erased by the retention period: one placeholder, no media and no actions.
    final expired = viewOnce ? const <Attachment>[] : m.attachments.where((a) => a.isExpired).toList();
    final live = viewOnce ? const <Attachment>[] : m.attachments.where((a) => !a.isExpired).toList();
    final images = live.where((a) => a.isImage).toList();
    final videos = live.where((a) => a.isVideo).toList();
    final voice = m.kind == 'voice' && live.isNotEmpty ? live.first : null;
    final files = live.where((a) => !a.isImage && !a.isVideo && a != voice).toList();
    final emojiOnly = m.kind == 'text' && !m.deleted && m.replyTo == null && (m.text?.isNotEmpty ?? false) && isEmojiOnly(m.text!);
    final media = !m.deleted && (images.isNotEmpty || videos.isNotEmpty || m.kind == 'location');
    final showName = group && !out && first;
    final overlayMeta = media && (m.text?.isEmpty ?? true) && m.kind != 'location';
    final hasReactions = !m.deleted && m.reactions.isNotEmpty;
    final innerPad = media ? const EdgeInsets.symmetric(horizontal: 8) : EdgeInsets.zero;

    final children = <Widget>[];
    if (showName) {
      children.add(Padding(
        padding: innerPad.add(const EdgeInsets.only(bottom: 3)),
        child: Text(m.senderName, style: ts(13, weight: 700, color: nameColor(hueFor(m.senderAccountId), dark: t.dark))),
      ));
    }
    if (m.forwarded && !m.deleted) {
      children.add(Padding(
        padding: innerPad.add(const EdgeInsets.only(bottom: 5)),
        child: Row(mainAxisSize: MainAxisSize.min, children: [
          AppIcon('forward', size: 13, color: c.meta),
          const SizedBox(width: 5),
          Text(context.t('message.forwarded'), style: ts(12.5, color: c.meta, fontStyle: FontStyle.italic)),
        ]),
      ));
    }
    if (m.deleted) {
      children.add(Row(mainAxisSize: MainAxisSize.min, children: [
        AppIcon('ban', size: 15, color: c.meta),
        const SizedBox(width: 6),
        Flexible(
          child: Text(context.t(out ? 'message.deletedByYou' : 'message.deleted'), style: ts(14.5, color: c.meta, fontStyle: FontStyle.italic)),
        ),
      ]));
      children.add(Align(alignment: Alignment.centerRight, child: _Meta(m: m, c: c)));
    } else {
      if (m.replyTo != null) {
        children.add(QuoteBlock(
          q: m.replyTo!,
          bg: c.replyBg,
          bar: c.replyBar,
          onTap: selecting ? null : () => store.set((d) => d.copyWith(highlightId: m.replyTo!.id)),
        ));
      }
      if (viewOnce) children.add(ViewOnceBody(m: m, fg: c.fg, meta: c.meta, accent: t.accent, enabled: !selecting));
      if (images.isNotEmpty) children.add(ImageGrid(message: m, attachments: images));
      for (final a in videos) {
        children.add(Padding(padding: const EdgeInsets.only(bottom: 2), child: VideoAttachment(message: m, a: a)));
      }
      if (voice != null) {
        children.add(VoicePlayer(message: m, a: voice, playBg: c.playBg, playFg: c.playFg, waveOn: c.waveOn, waveOff: c.waveOff, meta: c.meta));
      }
      for (final a in files) {
        children.add(FileCard(message: m, a: a, metaColor: c.meta, iconBg: c.replyBg));
      }
      if (expired.isNotEmpty) children.add(ExpiredMediaCard(attachments: expired, voice: m.kind == 'voice', metaColor: c.meta));
      if (m.kind == 'location' && m.location != null) children.add(_LocationBody(m: m, c: c));
      if (m.contacts.isNotEmpty) {
        children.add(Padding(
          padding: const EdgeInsets.symmetric(horizontal: 4),
          child: ContactCards(contacts: m.contacts, fg: c.fg, meta: c.meta, link: c.link),
        ));
      }
      if (m.text?.isNotEmpty ?? false) {
        children.add(Padding(
          padding: innerPad.add(EdgeInsets.only(top: media ? 7 : 0)),
          child: _RichText(
            text: m.text!,
            mentions: m.mentions,
            codeBg: c.replyBg,
            link: c.link,
            style: emojiOnly ? const TextStyle(fontSize: 42, height: 1.15) : ts(15, color: c.fg, height: 1.42),
          ),
        ));
      }
      if (!overlayMeta) {
        children.add(Padding(
          padding: innerPad.add(const EdgeInsets.only(top: 1)),
          child: Align(
            alignment: Alignment.centerRight,
            child: emojiOnly
                ? Container(
                    padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 2),
                    decoration: BoxDecoration(color: t.pane, borderRadius: BorderRadius.circular(999), border: Border.all(color: t.line)),
                    child: _Meta(m: m, c: BubbleColors(t, false)),
                  )
                : _Meta(m: m, c: c),
          ),
        ));
      }
    }

    Widget bubble = Container(
      padding: emojiOnly ? const EdgeInsets.symmetric(horizontal: 2) : media ? const EdgeInsets.fromLTRB(4, 4, 4, 6) : const EdgeInsets.fromLTRB(12, 8, 12, 6),
      decoration: emojiOnly
          ? null
          : BoxDecoration(
              color: c.gradient == null ? c.bg : null,
              gradient: c.gradient,
              borderRadius: BorderRadius.circular(t.rBubble),
              boxShadow: highlighted
                  ? [BoxShadow(color: t.accent.withValues(alpha: .35), spreadRadius: 4)]
                  : t.bubbleShadow,
            ),
      child: DefaultTextStyle.merge(
        style: TextStyle(color: c.fg),
        child: IconTheme.merge(
          data: IconThemeData(color: c.fg),
          child: Column(crossAxisAlignment: CrossAxisAlignment.start, mainAxisSize: MainAxisSize.min, children: children),
        ),
      ),
    );
    if (overlayMeta) {
      bubble = Stack(children: [bubble, Positioned(right: 10, bottom: 12, child: _Meta(m: m, c: c, overlay: true))]);
    }
    if (hasReactions) {
      bubble = Stack(clipBehavior: Clip.none, children: [
        bubble,
        Positioned(right: 12, bottom: -14, child: _ReactionsPill(m: m)),
      ]);
    }

    final reason = out ? _statusReason(context, ref, m) : null;
    final retry = m.status == 'failed' && out && m.statusReason != 'account_gone'
        ? GestureDetector(
            onTap: () => retryMessage(m),
            child: Padding(
              padding: const EdgeInsets.only(top: 4),
              child: Row(mainAxisSize: MainAxisSize.min, children: [
                AppIcon('alert', size: 13, color: t.danger),
                const SizedBox(width: 4),
                Text(context.t('message.notSent'), style: ts(12, color: t.danger)),
              ]),
            ),
          )
        : null;

    return LayoutBuilder(builder: (context, cons) {
      final maxW = cons.maxWidth * .84 - (selecting ? 36 : 0);
      final content = ConstrainedBox(
        constraints: BoxConstraints(maxWidth: maxW),
        child: Column(
          crossAxisAlignment: out ? CrossAxisAlignment.end : CrossAxisAlignment.start,
          mainAxisSize: MainAxisSize.min,
          children: [bubble, ?reason, ?retry],
        ),
      );
      return GestureDetector(
        behavior: HitTestBehavior.opaque,
        onTap: selecting ? () => toggleSelect(m.conversationId, m.id) : null,
        onLongPress: selecting
            ? null
            : () {
                HapticFeedback.selectionClick();
                showMessageActions(context, m);
              },
        child: Container(
          margin: EdgeInsets.only(top: first ? 10 : 3, bottom: hasReactions ? 16 : 0),
          decoration: selected
              ? BoxDecoration(color: t.accent.withValues(alpha: .12), borderRadius: BorderRadius.circular(10))
              : null,
          child: Row(children: [
            if (selecting) ...[CheckBox(on: selected, round: true), const SizedBox(width: 14)],
            Expanded(child: Align(alignment: out ? Alignment.centerRight : Alignment.centerLeft, child: content)),
          ]),
        ),
      );
    });
  }
}

class _LocationBody extends StatefulWidget {
  const _LocationBody({required this.m, required this.c});
  final Message m;
  final BubbleColors c;

  @override
  State<_LocationBody> createState() => _LocationBodyState();
}

class _LocationBodyState extends State<_LocationBody> {
  // Map tiles come from a third-party server, which would learn the area (and our IP):
  // only load them when asked.
  bool _showMap = false;

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    final m = widget.m;
    final c = widget.c;
    final loc = m.location!;
    return SizedBox(
      width: 290,
      child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
        ClipRRect(
          borderRadius: BorderRadius.circular((t.rBubble - 5).clamp(4, 40)),
          child: SizedBox(
            height: 132,
            child: _showMap
                ? IgnorePointer(child: LocationMap(lat: loc.lat, lng: loc.lng))
                : Material(
                    color: t.mapBg,
                    child: InkWell(
                      onTap: () => setState(() => _showMap = true),
                      child: Column(mainAxisAlignment: MainAxisAlignment.center, children: [
                        AppIcon('mappin', size: 22, color: t.muted),
                        const SizedBox(height: 6),
                        Text(context.t('location.showMap'), style: ts(13, weight: 600, color: t.muted)),
                      ]),
                    ),
                  ),
          ),
        ),
        Padding(
          padding: const EdgeInsets.fromLTRB(8, 10, 8, 2),
          child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
            Text((loc.name?.isNotEmpty ?? false) ? loc.name! : context.t('kinds.location'), style: ts(14.5, weight: 700, color: c.fg)),
            const SizedBox(height: 2),
            Text(
              (loc.address?.isNotEmpty ?? false) ? loc.address! : '${loc.lat.toStringAsFixed(5)}, ${loc.lng.toStringAsFixed(5)}',
              style: ts(12.5, color: c.meta),
            ),
            const SizedBox(height: 10),
            GestureDetector(
              onTap: () => launchUrl(Uri.parse(mapsUrl(loc.lat, loc.lng)), mode: LaunchMode.externalApplication),
              child: Row(mainAxisSize: MainAxisSize.min, children: [
                AppIcon('external', size: 15, color: c.link),
                const SizedBox(width: 6),
                Text(context.t('location.openInMaps'), style: ts(13.5, weight: 700, color: c.link)),
              ]),
            ),
          ]),
        ),
      ]),
    );
  }
}

/// Reaction pill hanging off the bottom right corner of the bubble ("👍 2").
/// Why an outgoing message is waiting (neutral: the recipient has no connected device) or failed
/// for good (the account was deleted; no retry).
Widget? _statusReason(BuildContext context, WidgetRef ref, Message m) {
  final t = context.tk;
  final (String icon, String text, Color color) row;
  if (m.status == 'pending' && m.statusReason == 'no_devices') {
    final name = ref.watch(storeProvider.select((s) => s.conversations[m.conversationId]?.title)) ?? '';
    row = ('clock', context.t('message.noDevices', args: {'name': name.isEmpty ? context.t('message.recipient') : name}), t.muted);
  } else if (m.status == 'failed' && m.statusReason == 'account_gone') {
    row = ('alert', context.t('message.accountGone'), t.danger);
  } else {
    return null;
  }
  return Padding(
    padding: const EdgeInsets.only(top: 4),
    child: Row(mainAxisSize: MainAxisSize.min, crossAxisAlignment: CrossAxisAlignment.start, children: [
      Padding(padding: const EdgeInsets.only(top: 1), child: AppIcon(row.$1, size: 13, color: row.$3)),
      const SizedBox(width: 4),
      Flexible(child: Text(row.$2, style: ts(12, color: row.$3, height: 1.35))),
    ]),
  );
}

class _ReactionsPill extends StatelessWidget {
  const _ReactionsPill({required this.m});
  final Message m;

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    final emojis = <String>{for (final r in m.reactions) r.emoji};
    final mine = m.reactions.any((r) => r.isMe);
    return GestureDetector(
      onTap: () => _showReactions(context, m),
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 2),
        decoration: BoxDecoration(
          color: t.pane,
          borderRadius: BorderRadius.circular(999),
          border: Border.all(color: mine ? t.accent : t.line),
          boxShadow: const [BoxShadow(color: Color.fromRGBO(0, 0, 0, .06), blurRadius: 6, offset: Offset(0, 2))],
        ),
        child: Text('${emojis.join()}${m.reactions.length > 1 ? ' ${m.reactions.length}' : ''}', style: ts(12.5, color: t.ink)),
      ),
    );
  }
}

void _showReactions(BuildContext context, Message m) {
  showModalBottomSheet<void>(
    context: context,
    builder: (ctx) {
      final t = ctx.tk;
      return SafeArea(
        top: false,
        child: Padding(
          padding: const EdgeInsets.fromLTRB(12, 14, 12, 12),
          child: Column(mainAxisSize: MainAxisSize.min, crossAxisAlignment: CrossAxisAlignment.stretch, children: [
            Padding(
              padding: const EdgeInsets.fromLTRB(8, 0, 8, 8),
              child: Text(ctx.t('message.reactions'), style: ts(13, weight: 600, color: t.muted)),
            ),
            for (final r in m.reactions)
              Tap(
                onTap: r.isMe
                    ? () {
                        Navigator.of(ctx).pop();
                        react(m, null);
                      }
                    : null,
                radius: BorderRadius.circular(12),
                padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 8),
                child: Row(children: [
                  Avatar(name: r.senderName, seed: r.accountId, size: 36),
                  const SizedBox(width: 12),
                  Expanded(
                    child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
                      Text(r.isMe ? ctx.t('common.you') : r.senderName, style: ts(14.5, weight: 600, color: t.ink)),
                      if (r.isMe) Text(ctx.t('message.tapToRemove'), style: ts(12, color: t.muted)),
                    ]),
                  ),
                  Text(r.emoji, style: const TextStyle(fontSize: 22)),
                ]),
              ),
          ]),
        ),
      );
    },
  );
}

bool _canEdit(Message m) =>
    m.outgoing && !m.deleted && !m.isViewOnce && m.kind != 'system' && m.kind != 'voice' && m.kind != 'location' && m.status != 'failed';

/// Long-press sheet: quick reactions + message actions.
void showMessageActions(BuildContext context, Message m) {
  final t = context.tk;
  // View-once media is never copied, forwarded, saved or shared.
  // Media erased by the retention period: nothing to forward or save (the core refuses too).
  final mediaExpired = m.attachments.any((a) => a.isExpired);
  final saveable = m.isViewOnce || mediaExpired ? null : m.attachments.where((a) => a.localPath != null).firstOrNull;
  final mine = m.reactions.where((r) => r.isMe).firstOrNull?.emoji;
  final header = m.deleted
      ? null
      : Builder(
          builder: (ctx) => Container(
            padding: const EdgeInsets.all(4),
            decoration: BoxDecoration(color: t.sunk, borderRadius: BorderRadius.circular(999)),
            child: Row(mainAxisAlignment: MainAxisAlignment.spaceAround, children: [
              for (final e in kQuickReactions)
                GestureDetector(
                  onTap: () {
                    Navigator.of(ctx).pop();
                    react(m, mine == e ? null : e);
                  },
                  child: Container(
                    width: 42,
                    height: 42,
                    alignment: Alignment.center,
                    decoration: BoxDecoration(shape: BoxShape.circle, color: mine == e ? t.accentSoft : Colors.transparent),
                    child: Text(e, style: const TextStyle(fontSize: 24)),
                  ),
                ),
              GestureDetector(
                onTap: () async {
                  Navigator.of(ctx).pop();
                  final e = await pickEmoji(context);
                  if (e != null) await react(m, e);
                },
                child: Container(
                  width: 42,
                  height: 42,
                  alignment: Alignment.center,
                  decoration: BoxDecoration(shape: BoxShape.circle, color: t.pane),
                  child: AppIcon('plus', size: 18, color: t.muted, semanticLabel: ctx.t('message.moreReactions')),
                ),
              ),
            ]),
          ),
        );
  showActionSheet(context, header: header, actions: [
    SheetAction(label: context.t('message.retry'), icon: 'refresh', hidden: m.status != 'failed', onTap: () => retryMessage(m)),
    SheetAction(
      label: context.t('message.reply'),
      icon: 'reply',
      hidden: m.deleted,
      onTap: () => store.setComposing(m.conversationId, replyTo: m, editing: null),
    ),
    SheetAction(
      label: context.t('message.copy'),
      icon: 'copy',
      hidden: m.deleted || m.isViewOnce || (m.text?.isEmpty ?? true),
      onTap: () async {
        await Clipboard.setData(ClipboardData(text: m.text ?? ''));
        notifyOk(tr('common.copied'));
      },
    ),
    SheetAction(label: context.t('message.forward'), icon: 'forward', hidden: m.deleted || m.isViewOnce || mediaExpired, onTap: () => showForwardSheet(context, [m.id])),
    SheetAction(
      label: context.t('message.edit'),
      icon: 'edit',
      hidden: !_canEdit(m),
      onTap: () => store.setComposing(m.conversationId, editing: m, replyTo: null),
    ),
    SheetAction(
      label: context.t('message.editHistory'),
      icon: 'clock',
      hidden: m.deleted || m.editedAt == null,
      onTap: () => showEditHistory(context, m.id),
    ),
    SheetAction(
      label: context.t('mobile.saveTo'),
      icon: 'download',
      hidden: m.deleted || saveable == null,
      onTap: () => saveAttachment(m, saveable!),
    ),
    SheetAction(
      label: context.t('mobile.share'),
      icon: 'share',
      hidden: m.deleted || saveable == null,
      onTap: () => shareAttachment(m, saveable!),
    ),
    SheetAction(
      label: context.t(m.pinnedUntil != null ? 'pin.unpin' : 'pin.action'),
      icon: m.pinnedUntil != null ? 'pinoff' : 'pin',
      hidden: m.deleted || m.kind == 'system',
      onTap: () => m.pinnedUntil != null ? pinMessage(m, null) : openPinDialog(context, m),
    ),
    SheetAction(
      label: context.t(m.starred ? 'star.unstar' : 'star.action'),
      icon: m.starred ? 'staroff' : 'star',
      hidden: m.deleted || m.kind == 'system',
      onTap: () => starMessage(m, !m.starred),
    ),
    SheetAction(label: context.t('message.info'), icon: 'info', hidden: !m.outgoing || m.deleted, onTap: () => showMessageInfo(context, m.id)),
    SheetAction(label: context.t('message.select'), icon: 'check', onTap: () => toggleSelect(m.conversationId, m.id)),
    SheetAction(
      label: context.t('message.delete'),
      icon: 'trash',
      danger: true,
      onTap: () => showDeleteDialog(context, m.conversationId, [m.id]),
    ),
  ]);
}

/// Pin durations offered (seconds): 24 hours, 7 days, 30 days.
const pinDurations = [86400, 604800, 2592000];

Future<void> openPinDialog(BuildContext context, Message m) async {
  final v = await showOptions(
    context,
    title: context.t('pin.title'),
    description: context.t('pin.hint'),
    initial: '${pinDurations[1]}',
    options: [
      ('${pinDurations[0]}', context.t('pin.h24')),
      ('${pinDurations[1]}', context.t('pin.d7')),
      ('${pinDurations[2]}', context.t('pin.d30')),
    ],
  );
  if (v != null) await pinMessage(m, int.parse(v));
}
