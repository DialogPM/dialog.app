// The chat's pinned messages (this device only), under the header: tap to jump to the shown
// one and move on to the next.

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

class PinnedBar extends ConsumerStatefulWidget {
  const PinnedBar({super.key, required this.conv});
  final Conversation conv;

  @override
  ConsumerState<PinnedBar> createState() => _PinnedBarState();
}

class _PinnedBarState extends ConsumerState<PinnedBar> {
  List<Message> _pins = const [];
  int _index = 0;
  int? _loadedVersion;

  Future<void> _load(int version) async {
    _loadedVersion = version;
    try {
      final list = await backend.pinnedMessages(widget.conv.id);
      if (mounted) setState(() => (_pins = list, _index = 0));
    } catch (_) {}
  }

  @override
  Widget build(BuildContext context) {
    final version = ref.watch(storeProvider.select((s) => s.pinsVersion));
    if (version != _loadedVersion) _load(version);
    if (_pins.isEmpty) return const SizedBox.shrink();
    final t = context.tk;
    final i = _index.clamp(0, _pins.length - 1);
    // Edits arrive as message events: prefer the live copy.
    final m = ref.watch(storeProvider.select((s) => s.messages[_pins[i].id])) ?? _pins[i];
    final who = m.outgoing ? context.t('common.you') : m.senderName;
    final label = _pins.length > 1 ? context.t('pin.barCount', args: {'index': i + 1, 'count': _pins.length}) : context.t('pin.bar');
    return Container(
      key: const ValueKey('pinned-bar'),
      decoration: BoxDecoration(color: t.pane, border: Border(bottom: BorderSide(color: t.line))),
      child: Row(children: [
        Expanded(
          child: Tap(
            onTap: () {
              store.set((d) => d.copyWith(highlightId: m.id));
              setState(() => _index = (i + 1) % _pins.length);
            },
            padding: const EdgeInsets.fromLTRB(14, 8, 8, 8),
            child: Row(children: [
              Column(mainAxisSize: MainAxisSize.min, children: [
                for (var k = 0; k < _pins.length; k++)
                  Container(
                    width: 3,
                    height: 26 / _pins.length,
                    margin: const EdgeInsets.symmetric(vertical: 1),
                    decoration: BoxDecoration(color: k == i ? t.accent : t.line, borderRadius: BorderRadius.circular(2)),
                  ),
              ]),
              const SizedBox(width: 10),
              AppIcon('pin', size: 17, color: t.muted),
              const SizedBox(width: 10),
              Expanded(
                child: Column(crossAxisAlignment: CrossAxisAlignment.start, mainAxisSize: MainAxisSize.min, children: [
                  Text(label, style: ts(12.5, weight: 650, color: t.accentText)),
                  Text(
                    '${widget.conv.isGroup ? '$who: ' : ''}${previewText(m.kind, m.text, context.lang)}',
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: ts(14, color: t.ink),
                  ),
                ]),
              ),
            ]),
          ),
        ),
        IconBtn('x', color: t.muted, tooltip: context.t('pin.unpin'), onPressed: () => pinMessage(m, null)),
      ]),
    );
  }
}
