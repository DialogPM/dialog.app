// Forward, delete and "message info" (port of MessageDialogs.tsx).

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
import '../../widgets/dialogs.dart';
import '../../widgets/ui.dart';
import '../chats/chat_list.dart';
import '../chats/contact_picker.dart';

void showForwardSheet(BuildContext context, List<String> ids) {
  if (ids.isEmpty) return;
  Navigator.of(context).push(MaterialPageRoute<void>(builder: (_) => _ForwardPage(ids: ids)));
}

class _ForwardPage extends ConsumerStatefulWidget {
  const _ForwardPage({required this.ids});
  final List<String> ids;

  @override
  ConsumerState<_ForwardPage> createState() => _ForwardPageState();
}

class _ForwardPageState extends ConsumerState<_ForwardPage> {
  final _query = TextEditingController();
  final _targets = <String>[];
  bool _busy = false;

  @override
  void dispose() {
    _query.dispose();
    super.dispose();
  }

  Future<void> _send() async {
    setState(() => _busy = true);
    final ok = await attempt(() async {
      await backend.forwardMessages(widget.ids, _targets);
      return true;
    });
    if (!mounted) return;
    setState(() => _busy = false);
    if (ok == true) {
      notifyOk(tr('message.forwardedTo', count: _targets.length));
      store.set((d) => d.copyWith(selection: null));
      Navigator.of(context).pop();
    }
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    final q = _query.text.trim().toLowerCase();
    final list = ref.watch(storeProvider.select((s) => s.conversations)).values.where((c) => !c.left && !c.blocked && (q.isEmpty || c.title.toLowerCase().contains(q))).toList()
      ..sort((a, b) => b.sortTime.compareTo(a.sortTime));
    return PaneScaffold(
      header: PaneHeader(title: context.t('message.forwardTitle', count: widget.ids.length), backIcon: 'x'),
      body: Column(children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(16, 12, 16, 8),
          child: SearchBox(controller: _query, hint: context.t('chats.search'), onChanged: (_) => setState(() {})),
        ),
        Expanded(
          child: ListView.builder(
            itemCount: list.length,
            itemBuilder: (_, i) {
              final c = list[i];
              final on = _targets.contains(c.id);
              return PickRow(
                leading: Avatar(name: c.title, path: c.avatarPath, seed: c.id, size: 40, group: c.isGroup),
                title: c.title,
                onTap: () => setState(() => on ? _targets.remove(c.id) : _targets.add(c.id)),
                trailing: CheckBox(on: on, round: true),
              );
            },
          ),
        ),
      ]),
      bottom: Container(
        padding: EdgeInsets.fromLTRB(16, 12, 16, 12 + MediaQuery.of(context).padding.bottom),
        decoration: BoxDecoration(color: t.pane, border: Border(top: BorderSide(color: t.line))),
        child: Btn(
          label: '${context.t('message.forward')}${_targets.isEmpty ? '' : ' (${_targets.length})'}',
          icon: 'send',
          height: 48,
          busy: _busy,
          onPressed: _targets.isEmpty ? null : _send,
        ),
      ),
    );
  }
}

Future<void> showDeleteDialog(BuildContext context, String conversationId, List<String> ids) async {
  final s = appData;
  final msgs = ids.map((id) => s.messages[id]).whereType<Message>().toList();
  final forEveryone = msgs.isNotEmpty && msgs.every((m) => m.outgoing && !m.deleted && m.kind != 'system');
  final choice = await showAppDialog<String>(
    context,
    title: context.t('message.deleteTitle', count: ids.length),
    builder: (ctx) => Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
      if (forEveryone) ...[
        _DangerSoftButton(label: ctx.t('message.deleteForEveryone'), onTap: () => Navigator.of(ctx).pop('everyone'), filled: true),
        const SizedBox(height: 8),
      ],
      _DangerSoftButton(label: ctx.t('message.deleteForMe'), onTap: () => Navigator.of(ctx).pop('me'), filled: !forEveryone),
      const SizedBox(height: 8),
      Btn(label: ctx.t('common.cancel'), kind: BtnKind.secondary, height: 44, onPressed: () => Navigator.of(ctx).pop()),
    ]),
  );
  if (choice == null) return;
  store.set((d) => d.copyWith(selection: null));
  await deleteMessages(ids, forEveryone: choice == 'everyone');
}

class _DangerSoftButton extends StatelessWidget {
  const _DangerSoftButton({required this.label, required this.onTap, required this.filled});
  final String label;
  final VoidCallback onTap;
  final bool filled;

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    return SizedBox(
      height: 44,
      child: Tap(
        onTap: onTap,
        color: filled ? t.danger.withValues(alpha: .11) : Colors.transparent,
        radius: t.inputRadius(44),
        child: Center(child: Text(label, style: ts(14, weight: 650, color: t.danger))),
      ),
    );
  }
}

/// "Edit history": every version of an edited message, newest first.
Future<void> showEditHistory(BuildContext context, String messageId) async {
  final List<EditVersion> versions;
  try {
    versions = await backend.getEditHistory(messageId);
  } catch (e) {
    notifyError(e);
    return;
  }
  if (!context.mounted || versions.isEmpty) return;
  final lang = context.lang;
  await showModalBottomSheet<void>(
    context: context,
    useSafeArea: true,
    isScrollControlled: true,
    builder: (ctx) {
      final t = ctx.tk;
      final list = versions.reversed.toList();
      return SafeArea(
        top: false,
        child: ConstrainedBox(
          constraints: BoxConstraints(maxHeight: MediaQuery.of(ctx).size.height * .85),
          child: ListView(shrinkWrap: true, padding: const EdgeInsets.fromLTRB(20, 16, 20, 16), children: [
            Text(ctx.t('message.editHistory'), style: ts(16, weight: 600, color: t.ink)),
            const SizedBox(height: 12),
            for (final (i, v) in list.indexed)
              Padding(
                padding: const EdgeInsets.only(bottom: 14),
                child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
                  Text(
                    i == 0 ? '${formatDateTime(v.at, lang)} · ${ctx.t('message.editCurrent')}' : formatDateTime(v.at, lang),
                    style: ts(12, color: t.muted),
                  ),
                  const SizedBox(height: 2),
                  Text(v.text, style: ts(15, color: t.ink, height: 1.4)),
                ]),
              ),
          ]),
        ),
      );
    },
  );
}

void showMessageInfo(BuildContext context, String messageId) {
  Navigator.of(context).push(MaterialPageRoute<void>(builder: (_) => _MessageInfoPage(messageId: messageId)));
}

class _MessageInfoPage extends StatefulWidget {
  const _MessageInfoPage({required this.messageId});
  final String messageId;

  @override
  State<_MessageInfoPage> createState() => _MessageInfoPageState();
}

class _MessageInfoPageState extends State<_MessageInfoPage> {
  MessageInfo? _info;

  @override
  void initState() {
    super.initState();
    backend.getMessageInfo(widget.messageId).then((i) {
      if (mounted) setState(() => _info = i);
    }).catchError((Object e) {
      notifyError(e);
      if (mounted) Navigator.of(context).pop();
    });
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    final info = _info;
    final lang = context.lang;
    Widget receipt(MessageReceipt r, int? at) => Padding(
          padding: const EdgeInsets.symmetric(vertical: 6),
          child: Row(children: [
            Avatar(name: r.displayName, path: r.avatarPath, seed: r.accountId, size: 36),
            const SizedBox(width: 12),
            Expanded(
              child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
                Text(r.displayName, maxLines: 1, overflow: TextOverflow.ellipsis, style: ts(14, weight: 500, color: t.ink)),
                Text(at != null ? formatDateTime(at, lang) : '—', style: ts(12, color: t.muted)),
              ]),
            ),
            StatusTicks(status: r.status, color: t.muted, readColor: t.accentText),
          ]),
        );
    Widget section(String icon, Color color, String title, List<Widget> rows) => Padding(
          padding: const EdgeInsets.only(bottom: 16),
          child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
            Row(children: [
              if (icon.isNotEmpty) ...[AppIcon(icon, size: 18, color: color), const SizedBox(width: 6)],
              Text(title, style: ts(14, weight: 600, color: t.ink)),
            ]),
            const SizedBox(height: 4),
            ...rows,
          ]),
        );
    return PaneScaffold(
      header: PaneHeader(title: context.t('message.info')),
      body: info == null
          ? const Spinner()
          : ListView(padding: const EdgeInsets.all(20), children: [
              Align(
                alignment: Alignment.centerRight,
                child: Container(
                  constraints: const BoxConstraints(maxWidth: 300),
                  padding: const EdgeInsets.fromLTRB(12, 8, 12, 6),
                  decoration: BoxDecoration(
                    color: t.bubbleOutGradient == null ? t.bubbleOut : null,
                    gradient: t.bubbleOutGradient,
                    borderRadius: BorderRadius.circular(t.rBubble),
                  ),
                  child: Column(crossAxisAlignment: CrossAxisAlignment.end, children: [
                    Text(
                      info.message.deleted ? context.t('message.deletedByYou') : previewText(info.message.kind, info.message.text, lang),
                      style: ts(15, color: t.bubbleOutFg, height: 1.42),
                    ),
                    Text(formatDateTime(info.message.sentAt, lang), style: ts(11.5, color: t.outMeta)),
                  ]),
                ),
              ),
              const SizedBox(height: 20),
              if (info.receipts.any((r) => r.status == 'read'))
                section('checks', t.accentText, context.t('message.readBy'), [
                  for (final r in info.receipts.where((r) => r.status == 'read')) receipt(r, r.readAt),
                ]),
              if (info.receipts.any((r) => r.status == 'delivered'))
                section('checks', t.muted, context.t('message.deliveredTo'), [
                  for (final r in info.receipts.where((r) => r.status == 'delivered')) receipt(r, r.deliveredAt),
                ]),
              if (info.receipts.any((r) => r.status != 'read' && r.status != 'delivered'))
                section('', t.muted, context.t('message.notDelivered'), [
                  for (final r in info.receipts.where((r) => r.status != 'read' && r.status != 'delivered')) receipt(r, null),
                ]),
              if (info.receipts.isEmpty) Text(context.t('message.noReceipts'), style: ts(14, color: t.muted)),
            ]),
    );
  }
}
