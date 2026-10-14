// Bottom-anchored message list (reversed ListView) with upward infinite scroll, day separators,
// the encryption notice, typing indicator, jump-to-message and a scroll-to-bottom button.

import 'dart:async';
import 'dart:ui' show PointMode;

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../app/globals.dart';
import '../../core/models.dart';
import '../../l10n/i18n.dart';
import '../../state/actions.dart';
import '../../state/store.dart';
import '../../theme/app_theme.dart';
import '../../util/format.dart';
import '../../widgets/app_icon.dart';
import '../../widgets/ui.dart';
import 'message_bubble.dart';

const _groupGapMs = 10 * 60000;

sealed class _Row {
  const _Row(this.key);
  final String key;
}

class _E2eRow extends _Row {
  const _E2eRow() : super('e2e');
}

class _DayRow extends _Row {
  const _DayRow(super.key, this.ts);
  final int ts;
}

class _MsgRow extends _Row {
  const _MsgRow(super.key, this.first);
  final bool first;
}

List<_Row> _buildRows(List<String> ids, Map<String, Message> messages, bool complete) {
  final rows = <_Row>[];
  if (complete && ids.isEmpty) rows.add(const _E2eRow());
  Message? prev;
  for (final id in ids) {
    final m = messages[id];
    if (m == null) continue;
    final newDay = prev == null || dayKey(prev.sentAt) != dayKey(m.sentAt);
    if (newDay) rows.add(_DayRow('day-${m.id}', m.sentAt));
    // The encryption notice sits under the first date, at the very start of the history.
    if (prev == null && complete) rows.add(const _E2eRow());
    final first = newDay ||
        prev.kind == 'system' ||
        prev.senderAccountId != m.senderAccountId ||
        prev.outgoing != m.outgoing ||
        m.sentAt - prev.sentAt > _groupGapMs;
    rows.add(_MsgRow(m.id, first));
    prev = m;
  }
  return rows;
}

/// Dotted wallpaper of direction A (radial dots on a 20px grid).
class _WallPainter extends CustomPainter {
  const _WallPainter(this.color);
  final Color color;

  @override
  void paint(Canvas canvas, Size size) {
    final p = Paint()
      ..color = color
      ..strokeWidth = 2.1
      ..strokeCap = StrokeCap.round;
    final pts = <Offset>[];
    for (double y = 10; y < size.height; y += 20) {
      for (double x = 10; x < size.width; x += 20) {
        pts.add(Offset(x, y));
      }
    }
    canvas.drawPoints(PointMode.points, pts, p);
  }

  @override
  bool shouldRepaint(_WallPainter old) => old.color != color;
}

class MessageList extends ConsumerStatefulWidget {
  const MessageList({super.key, required this.conversationId, required this.group});
  final String conversationId;
  final bool group;

  @override
  ConsumerState<MessageList> createState() => _MessageListState();
}

class _MessageListState extends ConsumerState<MessageList> {
  final _scroll = ScrollController();
  final _keys = <String, GlobalKey>{};
  bool _atBottom = true;
  String? _flash;
  bool _jumping = false;

  String get id => widget.conversationId;

  @override
  void initState() {
    super.initState();
    _scroll.addListener(_onScroll);
  }

  @override
  void dispose() {
    _scroll.dispose();
    super.dispose();
  }

  void _onScroll() {
    final p = _scroll.position;
    final atBottom = p.pixels < 80;
    if (atBottom != _atBottom) setState(() => _atBottom = atBottom);
    if (p.pixels > p.maxScrollExtent - 800) unawaited(loadOlder(id));
  }

  GlobalKey _keyFor(String messageId) => _keys.putIfAbsent(messageId, GlobalKey.new);

  /// Jump to a message (reply quote tap, search result): load older pages until it is found.
  Future<void> _jumpTo(String messageId) async {
    if (_jumping) return;
    _jumping = true;
    try {
      for (var i = 0; i < 25 && mounted; i++) {
        final cm = appData.convMessages[id];
        if (cm == null || !cm.loaded) {
          await Future<void>.delayed(const Duration(milliseconds: 150));
          continue;
        }
        if (cm.ids.contains(messageId)) break;
        if (!cm.hasMore) break;
        final n = await loadOlder(id);
        if (n == 0 && appData.convMessages[id]?.hasMore != true) break;
      }
      // Scroll towards older messages until the row is built, then center it.
      for (var i = 0; i < 60 && mounted; i++) {
        await WidgetsBinding.instance.endOfFrame;
        final ctx = _keys[messageId]?.currentContext;
        if (ctx != null) {
          if (!ctx.mounted) break;
          await Scrollable.ensureVisible(ctx, alignment: .5, duration: const Duration(milliseconds: 250));
          setState(() => _flash = messageId);
          Timer(const Duration(milliseconds: 1800), () => mounted ? setState(() => _flash = null) : null);
          break;
        }
        if (!_scroll.hasClients) break;
        final p = _scroll.position;
        if (p.pixels >= p.maxScrollExtent) {
          if (appData.convMessages[id]?.hasMore != true) break;
          await loadOlder(id);
        }
        _scroll.jumpTo((p.pixels + p.viewportDimension * .8).clamp(0, p.maxScrollExtent));
      }
    } finally {
      _jumping = false;
      store.set((d) => d.copyWith(highlightId: null));
    }
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    final cm = ref.watch(storeProvider.select((s) => s.convMessages[id]));
    final messages = ref.watch(storeProvider.select((s) => s.messages));
    final selection = ref.watch(storeProvider.select((s) => s.selection));
    final unread = ref.watch(storeProvider.select((s) => s.conversations[id]?.unreadCount ?? 0));
    ref.listen(storeProvider.select((s) => s.highlightId), (_, next) {
      if (next != null) unawaited(_jumpTo(next));
    });
    final pending = appData.highlightId;
    if (pending != null && !_jumping && cm?.loaded == true) {
      WidgetsBinding.instance.addPostFrameCallback((_) => _jumpTo(pending));
    }

    Widget body;
    if (cm == null || !cm.loaded) {
      body = const Center(child: Spinner());
    } else {
      final rows = _buildRows(cm.ids, messages, !cm.hasMore);
      final selecting = selection?.conversationId == id;
      final selectedIds = selecting ? selection!.ids : const <String>[];
      final lang = context.lang;
      // Reversed list: index 0 is the typing footer, then rows from the newest.
      body = ListView.builder(
        controller: _scroll,
        reverse: true,
        padding: const EdgeInsets.fromLTRB(12, 0, 12, 0),
        itemCount: rows.length + 2,
        itemBuilder: (context, index) {
          if (index == 0) return _TypingFooter(conversationId: id);
          if (index == rows.length + 1) {
            return cm.loading && cm.hasMore ? const Spinner(size: 20, padding: 12) : const SizedBox(height: 18);
          }
          final row = rows[rows.length - index];
          switch (row) {
            case _E2eRow():
              return Padding(
                padding: const EdgeInsets.only(top: 10, bottom: 6),
                child: SysNote(icon: 'lock', text: context.t('chat.e2eNotice')),
              );
            case _DayRow(ts: final dayTs):
              return Padding(
                padding: const EdgeInsets.only(top: 10, bottom: 2),
                child: Center(
                  child: Container(
                    padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 5),
                    decoration: BoxDecoration(color: t.pane, borderRadius: BorderRadius.circular(999), border: Border.all(color: t.line)),
                    child: Text(formatDaySeparator(dayTs, lang), style: ts(12, weight: 600, color: t.muted)),
                  ),
                ),
              );
            case _MsgRow(:final first):
              return KeyedSubtree(
                key: _keyFor(row.key),
                child: MessageRow(
                  id: row.key,
                  first: first,
                  group: widget.group,
                  selecting: selecting,
                  selected: selectedIds.contains(row.key),
                  highlighted: _flash == row.key,
                ),
              );
          }
        },
      );
    }

    return Stack(children: [
      Positioned.fill(
        child: RepaintBoundary(
          child: CustomPaint(
            painter: t.wallDot == null ? null : _WallPainter(t.wallDot!),
            child: ColoredBox(color: Colors.transparent, child: Container()),
          ),
        ),
      ),
      Positioned.fill(child: body),
      if (!_atBottom)
        Positioned(
          right: 16,
          bottom: 14,
          child: GestureDetector(
            onTap: () => _scroll.animateTo(0, duration: const Duration(milliseconds: 250), curve: Curves.easeOut),
            child: Container(
              width: 42,
              height: 42,
              decoration: BoxDecoration(
                color: t.pane,
                shape: BoxShape.circle,
                border: Border.all(color: t.line),
                boxShadow: const [BoxShadow(color: Color.fromRGBO(0, 0, 0, .12), blurRadius: 14, offset: Offset(0, 4))],
              ),
              child: Stack(clipBehavior: Clip.none, children: [
                Center(child: RotatedBox(quarterTurns: 1, child: AppIcon('chevron', size: 20, color: t.ink))),
                if (unread > 0)
                  Positioned(
                    top: -6,
                    right: -4,
                    child: Container(
                      constraints: const BoxConstraints(minWidth: 19),
                      height: 19,
                      padding: const EdgeInsets.symmetric(horizontal: 5),
                      decoration: BoxDecoration(color: t.accent, borderRadius: BorderRadius.circular(999)),
                      alignment: Alignment.center,
                      child: Text('$unread', style: ts(10.5, weight: 700, color: t.onAccent, height: 1)),
                    ),
                  ),
              ]),
            ),
          ),
        ),
    ]);
  }
}

/// System note (encryption notice, group events): centered, sys colors, small icon.
class SysNote extends StatelessWidget {
  const SysNote({super.key, required this.icon, required this.text});
  final String icon;
  final String text;

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    return Center(
      child: Container(
        constraints: const BoxConstraints(maxWidth: 440),
        padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 9),
        decoration: BoxDecoration(color: t.sysBg, borderRadius: BorderRadius.circular(12)),
        child: Row(mainAxisSize: MainAxisSize.min, crossAxisAlignment: CrossAxisAlignment.start, children: [
          Padding(padding: const EdgeInsets.only(top: 2), child: AppIcon(icon, size: 13, color: t.sysFg)),
          const SizedBox(width: 8),
          Flexible(child: Text(text, textAlign: TextAlign.center, style: ts(12.5, color: t.sysFg, height: 1.45))),
        ]),
      ),
    );
  }
}

class _TypingFooter extends ConsumerWidget {
  const _TypingFooter({required this.conversationId});
  final String conversationId;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final t = context.tk;
    final typing = ref.watch(storeProvider.select((s) => s.typing[conversationId]));
    final isGroup = ref.watch(storeProvider.select((s) => s.conversations[conversationId]?.isGroup ?? false));
    final names = typing?.values.map((x) => x.name).toList() ?? const <String>[];
    if (names.isEmpty) return const SizedBox(height: 22);
    var text = context.t('presence.typing');
    if (isGroup && names.length == 1) text = context.t('presence.typingName', args: {'name': names.first});
    if (isGroup && names.length > 1) text = context.t('presence.typingMany', count: names.length);
    return Padding(
      padding: const EdgeInsets.only(top: 8, bottom: 22),
      child: Align(
        alignment: Alignment.centerLeft,
        child: Container(
          padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 10),
          decoration: BoxDecoration(color: t.bubbleIn, borderRadius: BorderRadius.circular(t.rBubble), boxShadow: t.bubbleShadow),
          child: Text(text, style: ts(14, color: t.muted)),
        ),
      ),
    );
  }
}
