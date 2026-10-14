// Message requests and group invites from strangers (see crates/chat-core/src/engine/requests.rs).
// Until the user accepts, the core sends nothing back (no receipts, typing or profile) and
// downloads nothing automatically; this bar replaces the composer with the choices.

import 'dart:async';

import 'package:flutter/material.dart';

import '../../app/globals.dart';
import '../../core/models.dart';
import '../../l10n/i18n.dart';
import '../../state/actions.dart';
import '../../theme/app_theme.dart';
import '../../widgets/dialogs.dart';
import '../../widgets/ui.dart';

/// Downloads an attachment, asking first when it comes from a message request (nothing from a
/// stranger is fetched or opened without the user's say).
void downloadChecked(Message m, Attachment a, void Function() download) {
  final conv = appData.conversations[m.conversationId];
  final ctx = navigatorKey.currentContext;
  if (m.outgoing || conv == null || !conv.isRequest || ctx == null) {
    download();
    return;
  }
  unawaited(showConfirm(
    ctx,
    title: ctx.t('request.downloadTitle'),
    body: ctx.t('request.downloadBody'),
    confirmLabel: ctx.t('message.download'),
    danger: false,
  ).then((ok) {
    if (ok) download();
  }));
}

class RequestBar extends StatefulWidget {
  const RequestBar({super.key, required this.conv});
  final Conversation conv;

  @override
  State<RequestBar> createState() => _RequestBarState();
}

class _RequestBarState extends State<RequestBar> {
  bool _busy = false;

  Conversation get conv => widget.conv;

  Future<void> _accept() async {
    setState(() => _busy = true);
    final c = await attempt(() => backend.acceptConversation(conv.id));
    if (c != null) {
      store.upsertConversation(c);
      // Thumbnails were held back while it was a request.
      await loadMessages(conv.id);
    }
    if (mounted) setState(() => _busy = false);
  }

  Future<void> _removeLocally() async {
    await backend.deleteConversation(conv.id);
    store.removeConversation(conv.id);
  }

  Future<void> _block() async {
    final ok = await showConfirm(
      context,
      title: context.t('chat.block.title', args: {'name': conv.title}),
      body: context.t('request.blockBody'),
      confirmLabel: context.t('chat.block.confirm'),
    );
    if (!ok) return;
    await setBlocked(conv.peerAccountId!, true);
    await attempt(_removeLocally);
  }

  Future<void> _remove() async {
    final group = conv.isGroup;
    final ok = await showConfirm(
      context,
      title: context.t(group ? 'request.declineTitle' : 'chat.delete.title', args: {'name': conv.title}),
      body: context.t(group ? 'request.declineBody' : 'request.deleteBody'),
      confirmLabel: context.t(group ? 'request.decline' : 'chat.delete.confirm'),
    );
    if (!ok) return;
    await attempt(() async {
      // Declining a group invite leaves the group (the others see it), then forgets it.
      if (group) await backend.leaveGroup(conv.id);
      await _removeLocally();
    });
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    final group = conv.isGroup;
    return Container(
      padding: EdgeInsets.fromLTRB(16, 14, 16, 14 + MediaQuery.of(context).padding.bottom),
      decoration: BoxDecoration(color: t.pane, border: Border(top: BorderSide(color: t.line))),
      child: Column(mainAxisSize: MainAxisSize.min, children: [
        Text(
          context.t(group ? 'request.groupTitle' : 'request.title', args: {'name': conv.title}),
          textAlign: TextAlign.center,
          style: ts(14.5, weight: 650, color: t.ink),
        ),
        const SizedBox(height: 4),
        Text(context.t(group ? 'request.groupBody' : 'request.body'), textAlign: TextAlign.center, style: ts(13.5, color: t.muted, height: 1.45)),
        const SizedBox(height: 12),
        Row(children: [
          if (!group && conv.peerAccountId != null) ...[
            Expanded(child: Btn(label: context.t('request.block'), kind: BtnKind.ghost, height: 44, onPressed: _busy ? null : _block)),
            const SizedBox(width: 8),
          ],
          Expanded(
            child: Btn(label: context.t(group ? 'request.decline' : 'request.delete'), kind: BtnKind.secondary, height: 44, onPressed: _busy ? null : _remove),
          ),
          const SizedBox(width: 8),
          Expanded(child: Btn(label: context.t(group ? 'request.join' : 'request.accept'), height: 44, busy: _busy, onPressed: _accept)),
        ]),
      ]),
    );
  }
}
