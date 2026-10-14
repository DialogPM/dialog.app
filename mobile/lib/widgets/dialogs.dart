// Modal helpers: action sheets (menus on phones), confirmation dialogs, option lists.

import 'package:flutter/material.dart';

import '../l10n/i18n.dart';
import '../theme/app_theme.dart';
import 'app_icon.dart';
import 'ui.dart';

class SheetAction {
  const SheetAction({required this.label, required this.onTap, this.icon, this.danger = false, this.hidden = false});

  final String label;
  final String? icon;
  final VoidCallback onTap;
  final bool danger;
  final bool hidden;
}

/// Bottom action sheet (.action-sheet): handle, optional title/header, big rows.
Future<void> showActionSheet(BuildContext context, {required List<SheetAction> actions, String? title, Widget? header}) {
  return showModalBottomSheet<void>(
    context: context,
    useSafeArea: true,
    isScrollControlled: true,
    builder: (ctx) {
      final t = ctx.tk;
      return SafeArea(
        top: false,
        child: ConstrainedBox(
          constraints: BoxConstraints(maxHeight: MediaQuery.of(ctx).size.height * .85),
          child: SingleChildScrollView(
            child: Padding(
              padding: const EdgeInsets.only(top: 10, bottom: 8),
              child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, mainAxisSize: MainAxisSize.min, children: [
                Center(
                  child: Container(
                    width: 40,
                    height: 4,
                    margin: const EdgeInsets.only(bottom: 12),
                    decoration: BoxDecoration(color: t.line, borderRadius: BorderRadius.circular(2)),
                  ),
                ),
                if (title != null)
                  Padding(
                    padding: const EdgeInsets.fromLTRB(20, 0, 20, 8),
                    child: Text(title, maxLines: 1, overflow: TextOverflow.ellipsis, style: ts(13, weight: 600, color: t.muted)),
                  ),
                if (header != null) Padding(padding: const EdgeInsets.fromLTRB(12, 0, 12, 12), child: header),
                for (final a in actions.where((a) => !a.hidden))
                  Tap(
                    onTap: () {
                      Navigator.of(ctx).pop();
                      a.onTap();
                    },
                    child: Container(
                      constraints: const BoxConstraints(minHeight: 52),
                      padding: const EdgeInsets.symmetric(horizontal: 20),
                      child: Row(children: [
                        SizedBox(
                          width: 22,
                          child: a.icon == null ? null : Center(child: AppIcon(a.icon!, size: 19, color: a.danger ? t.danger : t.muted)),
                        ),
                        const SizedBox(width: 16),
                        Expanded(child: Text(a.label, style: ts(15.5, weight: 550, color: a.danger ? t.danger : t.ink))),
                      ]),
                    ),
                  ),
              ]),
            ),
          ),
        ),
      );
    },
  );
}

/// Dialog with a title and custom content, in the design's modal style.
Future<T?> showAppDialog<T>(BuildContext context, {String? title, required Widget Function(BuildContext ctx) builder}) {
  return showDialog<T>(
    context: context,
    builder: (ctx) {
      final t = ctx.tk;
      return Dialog(
        insetPadding: const EdgeInsets.symmetric(horizontal: 16, vertical: 24),
        child: ConstrainedBox(
          constraints: const BoxConstraints(maxWidth: 460),
          child: SingleChildScrollView(
            padding: const EdgeInsets.all(20),
            child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, mainAxisSize: MainAxisSize.min, children: [
              if (title != null) ...[
                Row(children: [
                  Expanded(child: Text(title, style: ts(18, weight: 700, color: t.ink))),
                  IconBtn('x', size: 36, iconSize: 18, bg: t.sunk, onPressed: () => Navigator.of(ctx).pop(), tooltip: ctx.t('common.close')),
                ]),
                const SizedBox(height: 16),
              ],
              builder(ctx),
            ]),
          ),
        ),
      );
    },
  );
}

/// Row of dialog buttons aligned to the end (.dialog-actions).
class DialogActions extends StatelessWidget {
  const DialogActions({super.key, required this.children, this.alignment = MainAxisAlignment.end});
  final List<Widget> children;
  final MainAxisAlignment alignment;

  @override
  Widget build(BuildContext context) => Padding(
        padding: const EdgeInsets.only(top: 4),
        child: Wrap(alignment: WrapAlignment.end, spacing: 8, runSpacing: 8, children: children),
      );
}

/// Asks for confirmation; returns true when confirmed.
Future<bool> showConfirm(
  BuildContext context, {
  required String title,
  String? body,
  required String confirmLabel,
  bool danger = true,
}) async {
  final ok = await showAppDialog<bool>(
    context,
    title: title,
    builder: (ctx) => Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
      if (body != null) ...[Text(body, style: ts(14, color: ctx.tk.muted, height: 1.45)), const SizedBox(height: 18)],
      DialogActions(children: [
        Btn(label: ctx.t('common.cancel'), kind: BtnKind.secondary, height: 42, expand: false, onPressed: () => Navigator.of(ctx).pop(false)),
        Btn(label: confirmLabel, kind: danger ? BtnKind.danger : BtnKind.primary, height: 42, expand: false, onPressed: () => Navigator.of(ctx).pop(true)),
      ]),
    ]),
  );
  return ok == true;
}

/// Radio list + OK/Cancel; returns the chosen value.
Future<String?> showOptions(
  BuildContext context, {
  required String title,
  String? description,
  required List<(String value, String label)> options,
  required String initial,
}) {
  return showAppDialog<String>(context, title: title, builder: (ctx) {
    var value = initial;
    return StatefulBuilder(builder: (ctx, setState) {
      final t = ctx.tk;
      return Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
        if (description != null) ...[Text(description, style: ts(13.5, color: t.muted, height: 1.45)), const SizedBox(height: 8)],
        for (final o in options)
          Tap(
            onTap: () => setState(() => value = o.$1),
            radius: BorderRadius.circular(10),
            padding: const EdgeInsets.symmetric(horizontal: 4, vertical: 11),
            child: Row(children: [
              RadioDot(on: value == o.$1),
              const SizedBox(width: 12),
              Text(o.$2, style: ts(14.5, weight: 550, color: t.ink)),
            ]),
          ),
        const SizedBox(height: 12),
        DialogActions(children: [
          Btn(label: ctx.t('common.cancel'), kind: BtnKind.secondary, height: 42, expand: false, onPressed: () => Navigator.of(ctx).pop()),
          Btn(label: ctx.t('common.ok'), height: 42, expand: false, onPressed: () => Navigator.of(ctx).pop(value)),
        ]),
      ]);
    });
  });
}

/// Single text field dialog (rename, edit description...). Returns the trimmed value.
Future<String?> showTextEdit(
  BuildContext context, {
  required String title,
  required String initial,
  String? label,
  String? description,
  bool multiline = false,
  int maxLength = 80,
  bool allowEmpty = false,
  Widget? extraAction,
}) {
  final c = TextEditingController(text: initial);
  return showAppDialog<String>(context, title: title, builder: (ctx) {
    return StatefulBuilder(builder: (ctx, setState) {
      final valid = allowEmpty || c.text.trim().isNotEmpty;
      return Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
        AppField(
          controller: c,
          label: label,
          description: description,
          autofocus: true,
          maxLength: maxLength,
          maxLines: multiline ? 4 : 1,
          onChanged: (_) => setState(() {}),
          onSubmitted: multiline || !valid ? null : (v) => Navigator.of(ctx).pop(v.trim()),
        ),
        const SizedBox(height: 18),
        Row(children: [
          ?extraAction,
          const Spacer(),
          Btn(label: ctx.t('common.cancel'), kind: BtnKind.secondary, height: 42, expand: false, onPressed: () => Navigator.of(ctx).pop()),
          const SizedBox(width: 8),
          Btn(label: ctx.t('common.save'), height: 42, expand: false, onPressed: valid ? () => Navigator.of(ctx).pop(c.text.trim()) : null),
        ]),
      ]);
    });
  });
}
