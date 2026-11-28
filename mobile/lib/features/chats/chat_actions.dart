// Dialogs and menu actions that apply to a whole conversation (chat list long press, conversation
// header menu, info pane). Port of app/src/features/chats/dialogs.tsx.

import 'dart:async';
import 'dart:io';

import 'package:flutter/material.dart';

import '../../app/globals.dart';
import '../../core/backend.dart' show parseError;
import '../../core/models.dart';
import '../../l10n/i18n.dart';
import '../../services/files.dart';
import '../../services/media.dart';
import '../../state/actions.dart';
import '../../theme/app_theme.dart';
import '../../util/chat_export.dart';
import '../../widgets/app_icon.dart';
import '../../widgets/dialogs.dart';
import '../../widgets/toast.dart';
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

const _exportPage = 500;

/// Every message of a conversation, oldest first (the order the conversation shows them in).
Future<List<Message>> loadAllMessages(String conversationId) async {
  final seen = <String>{};
  final all = <Message>[];
  int? before;
  while (true) {
    final page = await backend.getMessages(conversationId, before: before, limit: _exportPage);
    final fresh = page.where((m) => seen.add(m.id)).toList();
    all.addAll(fresh);
    if (page.length < _exportPage || fresh.isEmpty) break;
    // Pages are cut by arrival time; messages that arrived in the same millisecond as the
    // last one are asked for again rather than lost (duplicates are dropped above).
    before = page.map((m) => m.receivedAt).reduce((a, b) => a < b ? a : b) + 1;
  }
  return all.reversed.toList();
}

/// "Export chat": the file leaves the app unencrypted, so the dialog says so first. Returns
/// "text", "media" or null when cancelled.
Future<String?> _askExport(BuildContext context, Conversation conv) {
  return showAppDialog<String>(context, title: context.t('chat.export.title', args: {'name': conv.title}), builder: (ctx) {
    var value = 'text';
    return StatefulBuilder(builder: (ctx, setState) {
      final t = ctx.tk;
      Widget option(String v, String label, String hint) => Tap(
            onTap: () => setState(() => value = v),
            radius: BorderRadius.circular(10),
            padding: const EdgeInsets.symmetric(horizontal: 4, vertical: 10),
            child: Row(crossAxisAlignment: CrossAxisAlignment.start, children: [
              Padding(padding: const EdgeInsets.only(top: 1), child: RadioDot(on: value == v)),
              const SizedBox(width: 12),
              Expanded(
                child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
                  Text(label, style: ts(14.5, weight: 550, color: t.ink)),
                  const SizedBox(height: 2),
                  Text(hint, style: ts(12.5, color: t.muted, height: 1.4)),
                ]),
              ),
            ]),
          );
      return Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
        Container(
          padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 10),
          decoration: BoxDecoration(
            color: t.danger.withValues(alpha: 0.09),
            border: Border.all(color: t.danger.withValues(alpha: 0.35)),
            borderRadius: BorderRadius.circular(10),
          ),
          child: Row(crossAxisAlignment: CrossAxisAlignment.start, children: [
            AppIcon('alert', size: 18, color: t.danger),
            const SizedBox(width: 10),
            Expanded(child: Text(ctx.t('chat.export.warning'), style: ts(13.5, color: t.danger, height: 1.4))),
          ]),
        ),
        const SizedBox(height: 8),
        option('text', ctx.t('chat.export.textOnly'), ctx.t('chat.export.textOnlyHint')),
        option('media', ctx.t('chat.export.withMedia'), ctx.t('chat.export.withMediaHint')),
        const SizedBox(height: 12),
        DialogActions(children: [
          Btn(label: ctx.t('common.cancel'), kind: BtnKind.secondary, height: 42, expand: false, onPressed: () => Navigator.of(ctx).pop()),
          Btn(label: ctx.t('chat.export.confirm'), height: 42, expand: false, onPressed: () => Navigator.of(ctx).pop(value)),
        ]),
      ]);
    });
  });
}

/// Writes the conversation to a temporary file (text, or an archive with the media) and hands
/// it to the system's share sheet; the file is removed afterwards.
Future<void> exportChat(BuildContext context, Conversation conv) async {
  final lang = context.lang;
  final choice = await _askExport(context, conv);
  if (choice == null) return;
  final withMedia = choice == 'media';
  final base = exportBaseName(conv.title);
  notifyOk(translate(lang, 'chat.export.working'));
  File? out;
  try {
    final transcript = buildTranscript(conv, await loadAllMessages(conv.id), lang, withMedia: withMedia);
    out = await newExportFile('$base.${withMedia ? 'zip' : 'txt'}');
    final skipped = await backend.exportConversation(
      conversationId: conv.id,
      destPath: out.path,
      text: transcript.text,
      archive: withMedia,
      textName: '$base.txt',
      media: [for (final m in transcript.media) m.toJson()],
    );
    if (skipped > 0) notifyOk(translate(lang, 'chat.export.doneSkipped', args: {'count': skipped}));
    await shareExport(out, withMedia ? 'application/zip' : 'text/plain');
  } catch (e) {
    if (out != null) unawaited(discardExport(out));
    if (parseError(e).message == 'export_too_large') {
      showToast(translate(lang, 'chat.export.tooLarge'));
    } else {
      notifyError(e);
    }
  }
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
    SheetAction(label: context.t('chat.export.action'), icon: 'share', onTap: () => exportChat(context, conv)),
    if (conv.isGroup && !conv.left)
      SheetAction(label: context.t('chat.clear.action'), icon: 'eraser', danger: true, onTap: () => confirmClearChat(context, conv))
    else
      SheetAction(label: context.t('chat.delete.action'), icon: 'trash', danger: true, onTap: () => confirmDeleteChat(context, conv)),
  ];
}
