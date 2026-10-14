// Emoji picker: recently used + categories (no external dependency, like the desktop picker).

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../l10n/i18n.dart';
import '../state/prefs.dart';
import '../theme/app_theme.dart';
import 'emoji_data.dart';
import 'ui.dart';

class EmojiPicker extends ConsumerStatefulWidget {
  const EmojiPicker({super.key, required this.onPick, this.height = 300});
  final ValueChanged<String> onPick;
  final double height;

  @override
  ConsumerState<EmojiPicker> createState() => _EmojiPickerState();
}

class _EmojiPickerState extends ConsumerState<EmojiPicker> {
  String _cat = 'recent';

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    final recent = ref.watch(prefsProvider.select((p) => p.recentEmoji));
    final list = _cat == 'recent' ? recent : kEmojiCategories.firstWhere((c) => c.key == _cat).emojis;
    Widget tab(String key, String icon) => GestureDetector(
          onTap: () => setState(() => _cat = key),
          child: Container(
            width: 40,
            height: 36,
            alignment: Alignment.center,
            decoration: BoxDecoration(color: _cat == key ? t.accentSoft : Colors.transparent, borderRadius: BorderRadius.circular(10)),
            child: Text(icon, style: const TextStyle(fontSize: 19)),
          ),
        );
    return SizedBox(
      height: widget.height,
      child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
        SizedBox(
          height: 44,
          child: ListView(scrollDirection: Axis.horizontal, padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 4), children: [
            tab('recent', '🕘'),
            for (final c in kEmojiCategories) tab(c.key, c.icon),
          ]),
        ),
        Padding(
          padding: const EdgeInsets.fromLTRB(14, 4, 14, 4),
          child: Text(context.t(_cat == 'recent' ? 'emoji.recent' : 'emoji.$_cat'), style: ts(12, weight: 600, color: t.muted)),
        ),
        Expanded(
          child: list.isEmpty
              ? Center(child: Text(context.t('emoji.noRecent'), style: ts(13, color: t.muted)))
              : GridView.builder(
                  padding: const EdgeInsets.symmetric(horizontal: 8),
                  gridDelegate: const SliverGridDelegateWithMaxCrossAxisExtent(maxCrossAxisExtent: 44),
                  itemCount: list.length,
                  itemBuilder: (_, i) => InkWell(
                    borderRadius: BorderRadius.circular(10),
                    onTap: () {
                      ref.read(prefsProvider.notifier).addRecentEmoji(list[i]);
                      widget.onPick(list[i]);
                    },
                    child: Center(child: Text(list[i], style: const TextStyle(fontSize: 26))),
                  ),
                ),
        ),
      ]),
    );
  }
}

/// Opens the picker in a bottom sheet and returns the chosen emoji.
Future<String?> pickEmoji(BuildContext context) => showModalBottomSheet<String>(
      context: context,
      useSafeArea: true,
      builder: (ctx) => SafeArea(
        top: false,
        child: Padding(
          padding: const EdgeInsets.only(top: 8),
          child: EmojiPicker(height: 340, onPick: (e) => Navigator.of(ctx).pop(e)),
        ),
      ),
    );
