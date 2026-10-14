// Dialogs and menu actions that apply to a whole conversation (chat list long press, conversation
// header menu, info pane). Port of app/src/features/chats/dialogs.tsx.

import 'package:flutter/material.dart';

import '../../app/globals.dart';
import '../../core/models.dart';
import '../../l10n/i18n.dart';
import '../../state/actions.dart';
import '../../widgets/dialogs.dart';
import '../../widgets/ui.dart';
import '../../util/format.dart' show kDisappearingOptions;

const _hour = 3600000;

Future<void> openMuteDialog(BuildContext context, Conversation conv) async {
  final v = await showOptions(
    context,
    title: context.t('chat.mute.title'),
    description: context.t('chat.mute.description'),
    initial: '8h',
    options: [('8h', context.t('chat.mute.h8')), ('1w', context.t('chat.mute.w1')), ('always', context.t('chat.mute.always'))],
  );
  if (v == null) return;
  final now = DateTime.now().millisecondsSinceEpoch;
  final until = v == 'always' ? -1 : now + (v == '8h' ? 8 * _hour : 7 * 24 * _hour);
  await patchConversation(conv.id, ConversationSettingsPatch(setMuted: true, mutedUntil: until));
}

Future<void> toggleMute(BuildContext context, Conversation conv) async {
  if (conv.isMuted()) {
    await patchConversation(conv.id, const ConversationSettingsPatch(setMuted: true, mutedUntil: null));
  } else {
    await openMuteDialog(context, conv);
  }
}

Future<void> openDisappearingDialog(BuildContext context, Conversation conv) async {
  final v = await showOptions(
    context,
    title: context.t('disappearing.title'),
    description: context.t('disappearing.description'),
    initial: '${conv.disappearingSeconds}',
    options: [for (final (secs, key) in kDisappearingOptions) ('$secs', context.t('disappearing.$key'))],
  );
  if (v == null) return;
  await patchConversation(conv.id, ConversationSettingsPatch(disappearingSeconds: int.parse(v)));
}

/// Asks for the account password (checked) before a destructive "for everyone" action;
/// returns it, or null when cancelled. The core checks it again.
Future<String?> _askPassword(BuildContext context, {required String title, required String confirmLabel}) {
  final pw = TextEditingController();
  return showAppDialog<String>(context, title: title, builder: (ctx) {
    String? error;
    var busy = false;
    return StatefulBuilder(builder: (ctx, setState) {
      Future<void> submit() async {
        setState(() {
          busy = true;
          error = null;
        });
        try {
          if (await backend.verifyPassword(pw.text)) {
            if (ctx.mounted) Navigator.of(ctx).pop(pw.text);
            return;
          }
          setState(() => error = tr('password.wrong'));
        } catch (e) {
          setState(() => error = errorText(e));
        } finally {
          if (ctx.mounted) setState(() => busy = false);
        }
      }

      return Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
        AppField(
          controller: pw,
          label: ctx.t('password.confirmAction'),
          password: true,
          error: error,
          onChanged: (_) => setState(() {}),
          onSubmitted: (_) => pw.text.isEmpty ? null : submit(),
        ),
        const SizedBox(height: 18),
        DialogActions(children: [
          Btn(label: ctx.t('common.cancel'), kind: BtnKind.secondary, height: 42, expand: false, onPressed: () => Navigator.of(ctx).pop()),
          Btn(label: confirmLabel, kind: BtnKind.danger, height: 42, expand: false, busy: busy, onPressed: pw.text.isEmpty ? null : submit),
        ]),
      ]);
    });
  });
}

/// Deletes the chat (only here, or also for the other side); returns true when deleted.
Future<bool> confirmDeleteChat(BuildContext context, Conversation conv) async {
  // An active group can only be cleared: deleting it would lose the group.
  if (conv.isGroup && !conv.left) {
    await confirmClearChat(context, conv);
    return false;
  }
  final self = !conv.isGroup && conv.peerAccountId == appData.appState?.account?.accountId;
  var everyone = false;
  if ((conv.isGroup && conv.left) || self) {
    final ok = await showConfirm(
      context,
      title: context.t('chat.delete.title', args: {'name': conv.title}),
      body: context.t('chat.delete.body'),
      confirmLabel: context.t('chat.delete.confirm'),
    );
    if (!ok) return false;
  } else {
    final v = await showOptions(
      context,
      title: context.t('chat.delete.title', args: {'name': conv.title}),
      description: context.t('chat.delete.forBothHint', args: {'name': conv.title}),
      initial: 'me',
      options: [
        ('me', context.t('chat.delete.forMe')),
        ('everyone', context.t('chat.delete.forBoth', args: {'name': conv.title})),
      ],
    );
    if (v == null) return false;
    everyone = v == 'everyone';
  }
  final password = everyone && context.mounted
      ? await _askPassword(context,
          title: context.t('chat.delete.title', args: {'name': conv.title}), confirmLabel: context.t('chat.delete.confirm'))
      : null;
  if (everyone && password == null) return false;
  final done = await attempt(() async {
    if (password != null) {
      await backend.deleteConversationForEveryone(conv.id, password);
    } else {
      await backend.deleteConversation(conv.id);
    }
    return true;
  });
  if (done == true) store.removeConversation(conv.id);
  return done == true;
}

Future<void> confirmClearChat(BuildContext context, Conversation conv) async {
  final group = conv.isGroup && !conv.left;
  var everyone = false;
  // Group admins can also clear the history for every member.
  if (group && appData.groups[conv.id]?.myRole == 'admin') {
    final v = await showOptions(
      context,
      title: context.t('chat.clear.title'),
      description: context.t('chat.clear.forGroupHint'),
      initial: 'me',
      options: [('me', context.t('chat.clear.forMe')), ('everyone', context.t('chat.clear.forGroup'))],
    );
    if (v == null) return;
    everyone = v == 'everyone';
  } else {
    final ok = await showConfirm(
      context,
      title: context.t('chat.clear.title'),
      body: context.t(group ? 'chat.clear.groupBody' : 'chat.clear.body'),
      confirmLabel: context.t('chat.clear.confirm'),
    );
    if (!ok) return;
  }
  final password = everyone && context.mounted
      ? await _askPassword(context, title: context.t('chat.clear.title'), confirmLabel: context.t('chat.clear.confirm'))
      : null;
  if (everyone && password == null) return;
  final done = await attempt(() async {
    if (password != null) {
      await backend.clearGroupForEveryone(conv.id, password);
    } else {
      await backend.clearConversation(conv.id);
    }
    return true;
  });
  if (done != true) return;
  store.clearConvMessages(conv.id);
  // Clearing for everyone leaves a notice.
  if (everyone) await loadMessages(conv.id);
}

Future<void> confirmBlock(BuildContext context, String accountId, String name, bool block) async {
  if (!block) {
    await setBlocked(accountId, false);
    return;
  }
  final ok = await showConfirm(
    context,
    title: context.t('chat.block.title', args: {'name': name}),
    body: context.t('chat.block.body'),
    confirmLabel: context.t('chat.block.confirm'),
  );
  if (ok) await setBlocked(accountId, true);
}

/// Reports the account to the server for review and blocks it.
Future<void> confirmReportSpam(BuildContext context, String accountId) async {
  final ok = await showConfirm(
    context,
    title: context.t('security.reportTitle'),
    body: context.t('security.reportBody'),
    confirmLabel: context.t('security.reportSpam'),
  );
  if (!ok || !context.mounted) return;
  final reported = context.t('security.reported');
  final c = await attempt(() => backend.reportSpam(accountId));
  if (c != null) {
    store.upsertContact(c);
    notifyOk(reported);
  }
}

Future<void> confirmLeaveGroup(BuildContext context, Conversation conv) async {
  final ok = await showConfirm(
    context,
    title: context.t('group.leave.title', args: {'name': conv.title}),
    body: context.t('group.leave.body'),
    confirmLabel: context.t('group.leave.confirm'),
  );
  if (ok) await attempt(() => backend.leaveGroup(conv.id));
}

/// Actions of the chat list long-press sheet.
List<SheetAction> listActions(BuildContext context, Conversation conv) {
  final muted = conv.isMuted();
  return [
    SheetAction(
      label: context.t(conv.archived ? 'chat.unarchive' : 'chat.archive'),
      icon: conv.archived ? 'unarchive' : 'archive',
      onTap: () => patchConversation(conv.id, ConversationSettingsPatch(archived: !conv.archived, pinned: conv.archived ? conv.pinned : false)),
    ),
    SheetAction(
      label: context.t(conv.pinned ? 'chat.unpin' : 'chat.pin'),
      icon: conv.pinned ? 'pinoff' : 'pin',
      hidden: conv.archived,
      onTap: () => patchConversation(conv.id, ConversationSettingsPatch(pinned: !conv.pinned)),
    ),
    SheetAction(
      label: context.t(muted ? 'chat.unmute' : 'chat.muteAction'),
      icon: muted ? 'bell' : 'mute',
      onTap: () => toggleMute(context, conv),
    ),
    SheetAction(
      label: context.t('chat.markRead'),
      icon: 'check',
      hidden: conv.unreadCount == 0,
      onTap: () => attempt(() => backend.markRead(conv.id)),
    ),
    if (conv.isGroup && !conv.left)
      SheetAction(label: context.t('chat.clear.action'), icon: 'eraser', danger: true, onTap: () => confirmClearChat(context, conv))
    else
      SheetAction(label: context.t('chat.delete.action'), icon: 'trash', danger: true, onTap: () => confirmDeleteChat(context, conv)),
  ];
}
