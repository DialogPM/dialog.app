// Conversation pane: header (or selection bar / in-chat search), messages, composer.

import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../app/globals.dart';
import '../../app/nav.dart';
import '../../core/models.dart';
import '../../l10n/i18n.dart';
import '../../services/notifications.dart';
import '../../state/calls.dart' show startCall;
import '../../state/group_calls.dart' show startGroupCall;
import '../calls/group_call_screen.dart' show GroupCallBanner;
import '../../state/actions.dart';
import '../../state/store.dart';
import '../../theme/app_theme.dart';
import '../../util/format.dart';
import '../../widgets/app_icon.dart';
import '../../widgets/dialogs.dart';
import '../../widgets/ui.dart';
import '../chats/chat_actions.dart';
import '../chats/chat_list.dart';
import '../info/info_page.dart';
import 'composer.dart';
import 'message_dialogs.dart';
import 'message_list.dart';
import 'pinned_bar.dart';
import 'request_bar.dart';
import '../groups/invite_pages.dart';
import 'subtitle.dart';

/// Lets the info pane start the in-chat search when it pops back to the conversation.
final chatSearchRequest = ValueNotifier<String?>(null);

class ConversationPage extends ConsumerStatefulWidget {
  const ConversationPage({super.key, required this.conversationId});
  final String conversationId;

  @override
  ConsumerState<ConversationPage> createState() => _ConversationPageState();
}

class _ConversationPageState extends ConsumerState<ConversationPage> {
  bool _searching = false;

  String get id => widget.conversationId;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) {
      store.set((d) => d.copyWith(activeId: id, selection: null));
      unawaited(Notifications.instance.cancelFor(id));
    });
    chatSearchRequest.addListener(_onSearchRequest);
  }

  @override
  void dispose() {
    chatSearchRequest.removeListener(_onSearchRequest);
    final s = appData;
    if (s.activeId == id) {
      // Deferred: the store must not change while the tree is being finalized.
      Future.microtask(() => store.set((d) => d.copyWith(activeId: null, selection: null, highlightId: null)));
    }
    super.dispose();
  }

  void _onSearchRequest() {
    if (chatSearchRequest.value == id) {
      chatSearchRequest.value = null;
      setState(() => _searching = true);
    }
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    final conv = ref.watch(storeProvider.select((s) => s.conversations[id]));
    final selecting = ref.watch(storeProvider.select((s) => s.selection?.conversationId == id));
    if (conv == null) {
      // Deleted while open.
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (mounted) Navigator.of(context).maybePop();
      });
      return Scaffold(backgroundColor: t.pane);
    }
    final block = _writeBlock(conv);
    return PopScope(
      canPop: !selecting && !_searching,
      onPopInvokedWithResult: (didPop, _) {
        if (didPop) return;
        if (selecting) store.set((d) => d.copyWith(selection: null));
        if (_searching) setState(() => _searching = false);
      },
      child: Scaffold(
        backgroundColor: t.pane,
        resizeToAvoidBottomInset: true,
        body: SafeArea(
          bottom: false,
          child: Column(children: [
            if (selecting)
              _SelectionBar(conversationId: id)
            else if (_searching)
              _ChatSearchBar(onClose: () => setState(() => _searching = false))
            else
              _ChatHeader(conv: conv, onSearch: () => setState(() => _searching = true)),
            if (!selecting && !_searching) PinnedBar(conv: conv),
            if (!selecting && !_searching && conv.isGroup && !conv.left) GroupCallBanner(conversationId: conv.id),
            if (!selecting && !_searching && conv.isGroup && !conv.left) JoinRequestsBanner(conversationId: conv.id),
            Expanded(
              child: Stack(children: [
                Positioned.fill(child: MessageList(conversationId: id, group: conv.isGroup)),
                if (_searching) Positioned.fill(child: _ChatSearchResults(conv: conv, onPick: () => setState(() => _searching = false))),
              ]),
            ),
            if (block != null)
              _BlockedNotice(conv: conv, reason: block)
            // Message request / group invite from a stranger: accept (or not) before writing.
            else if (conv.isRequest)
              RequestBar(key: ValueKey('request-${conv.id}'), conv: conv)
            else
              Composer(key: ValueKey(conv.id), conv: conv),
          ]),
        ),
      ),
    );
  }

  /// Why the composer is replaced by a notice, or null when the user can write.
  String? _writeBlock(Conversation conv) {
    final blocked = ref.watch(storeProvider.select((s) => conv.peerAccountId != null ? (s.contacts[conv.peerAccountId]?.blocked ?? conv.blocked) : conv.blocked));
    final onlyAdmins = ref.watch(storeProvider.select((s) => s.groups[conv.id]?.settings.onlyAdminsSend ?? false));
    if (!conv.isGroup && blocked) return 'blocked';
    if (conv.isGroup && conv.left) return 'left';
    if (conv.isGroup && onlyAdmins && !conv.isAdmin) return 'admins';
    return null;
  }
}

class _BlockedNotice extends StatelessWidget {
  const _BlockedNotice({required this.conv, required this.reason});
  final Conversation conv;
  final String reason;

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    return Container(
      padding: EdgeInsets.fromLTRB(16, 14, 16, 14 + MediaQuery.of(context).padding.bottom),
      decoration: BoxDecoration(color: t.pane, border: Border(top: BorderSide(color: t.line))),
      child: Column(mainAxisSize: MainAxisSize.min, children: [
        Text(context.t('chat.cannotWrite.$reason'), textAlign: TextAlign.center, style: ts(13.5, color: t.muted, height: 1.45)),
        if (reason == 'blocked' && conv.peerAccountId != null) ...[
          const SizedBox(height: 6),
          LinkBtn(label: context.t('chat.block.unblock'), onPressed: () => confirmBlock(context, conv.peerAccountId!, conv.title, false)),
        ],
      ]),
    );
  }
}

class _ChatHeader extends ConsumerWidget {
  const _ChatHeader({required this.conv, required this.onSearch});
  final Conversation conv;
  final VoidCallback onSearch;

  List<SheetAction> _actions(BuildContext context, bool blocked) {
    final muted = conv.isMuted();
    return [
      SheetAction(label: context.t('chat.searchIn'), icon: 'search', onTap: onSearch),
      SheetAction(label: context.t(conv.isGroup ? 'group.info' : 'contact.info'), icon: 'info', onTap: () => openInfo(conv.id)),
      SheetAction(
        label: context.t('contact.rename'),
        icon: 'user',
        hidden: conv.isGroup || conv.peerAccountId == null || appData.contacts[conv.peerAccountId] == null,
        onTap: () => renameContactDialog(context, appData.contacts[conv.peerAccountId]!),
      ),
      SheetAction(
        label: context.t('chat.selectMessages'),
        icon: 'check',
        onTap: () => store.set((d) => d.copyWith(selection: Selection(conv.id, const []))),
      ),
      SheetAction(label: context.t(muted ? 'chat.unmute' : 'chat.muteAction'), icon: muted ? 'bell' : 'mute', onTap: () => toggleMute(context, conv)),
      SheetAction(
        label: context.t('disappearing.title'),
        icon: 'clock',
        // Group timer: a group rule only admins change.
        hidden: conv.left || (conv.isGroup && appData.groups[conv.id]?.myRole != 'admin'),
        onTap: () => openDisappearingDialog(context, conv),
      ),
      SheetAction(
        label: context.t(conv.pinned ? 'chat.unpin' : 'chat.pin'),
        icon: conv.pinned ? 'pinoff' : 'pin',
        onTap: () => patchConversation(conv.id, ConversationSettingsPatch(pinned: !conv.pinned)),
      ),
      SheetAction(
        label: context.t(conv.archived ? 'chat.unarchive' : 'chat.archive'),
        icon: conv.archived ? 'unarchive' : 'archive',
        onTap: () => patchConversation(conv.id, ConversationSettingsPatch(archived: !conv.archived)),
      ),
      SheetAction(label: context.t('chat.clear.action'), icon: 'eraser', onTap: () => confirmClearChat(context, conv)),
      SheetAction(
        label: context.t(blocked ? 'chat.block.unblock' : 'chat.block.action'),
        icon: 'ban',
        danger: !blocked,
        hidden: conv.isGroup || conv.peerAccountId == null,
        onTap: () => confirmBlock(context, conv.peerAccountId!, conv.title, !blocked),
      ),
      SheetAction(
        label: context.t('security.reportSpam'),
        icon: 'alert',
        danger: true,
        hidden: conv.isGroup || conv.peerAccountId == null,
        onTap: () => confirmReportSpam(context, conv.peerAccountId!),
      ),
      SheetAction(label: context.t('group.leave.action'), icon: 'logout', danger: true, hidden: !conv.isGroup || conv.left, onTap: () => confirmLeaveGroup(context, conv)),
      SheetAction(
        label: context.t('chat.delete.action'),
        icon: 'trash',
        danger: true,
        // Groups: leave first (clearing is above).
        hidden: conv.isGroup && !conv.left,
        onTap: () => confirmDeleteChat(context, conv),
      ),
    ];
  }

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final t = context.tk;
    final sub = conversationSubtitle(context, ref, conv);
    final verified = ref.watch(storeProvider.select((s) => conv.peerAccountId == null ? false : (s.contacts[conv.peerAccountId]?.verified ?? false)));
    final blocked = ref.watch(storeProvider.select((s) => conv.peerAccountId != null ? (s.contacts[conv.peerAccountId]?.blocked ?? conv.blocked) : conv.blocked));
    return Container(
      height: 66,
      padding: const EdgeInsets.only(left: 6, right: 12),
      decoration: BoxDecoration(color: t.pane, border: Border(bottom: BorderSide(color: t.line))),
      child: Row(children: [
        IconBtn('back', iconSize: 21, tooltip: context.t('common.back'), onPressed: () => Navigator.of(context).maybePop()),
        const SizedBox(width: 6),
        Expanded(
          child: Semantics(
            button: true,
            label: context.t(conv.isGroup ? 'group.info' : 'contact.info'),
            child: GestureDetector(
              behavior: HitTestBehavior.opaque,
              onTap: () => openInfo(conv.id),
              child: Row(children: [
                Avatar(name: conv.title, path: conv.avatarPath, seed: conv.id, size: 42, group: conv.isGroup),
                const SizedBox(width: 12),
                Expanded(
                  child: Column(crossAxisAlignment: CrossAxisAlignment.start, mainAxisAlignment: MainAxisAlignment.center, children: [
                    Row(children: [
                      Flexible(child: Text(conv.title, maxLines: 1, overflow: TextOverflow.ellipsis, style: ts(16, weight: 700, color: t.ink))),
                      if (verified) ...[const SizedBox(width: 6), AppIcon('shield', size: 15, color: t.accentText)],
                      if (conv.disappearingSeconds > 0) ...[
                        const SizedBox(width: 6),
                        // Disappearing messages on: the timer and its duration next to the name.
                        Semantics(
                          label: '${context.t('disappearing.title')}: ${disappearingLabel(conv.disappearingSeconds, context.lang)}',
                          child: Container(
                            key: const ValueKey('timer-chip'),
                            padding: const EdgeInsets.fromLTRB(5, 1, 7, 1),
                            decoration: BoxDecoration(color: t.sunk, borderRadius: BorderRadius.circular(999)),
                            child: Row(mainAxisSize: MainAxisSize.min, children: [
                              AppIcon('timer', size: 12, color: t.muted),
                              const SizedBox(width: 3),
                              Text(disappearingLabel(conv.disappearingSeconds, context.lang), style: ts(11.5, weight: 600, color: t.muted)),
                            ]),
                          ),
                        ),
                      ],
                    ]),
                    Text(sub.text, maxLines: 1, overflow: TextOverflow.ellipsis, style: ts(12.5, color: sub.accent ? t.accentText : t.muted)),
                  ]),
                ),
              ]),
            ),
          ),
        ),
        if (conv.isGroup && !conv.left) ...[
          IconBtn('video', color: t.muted, iconSize: 21, tooltip: context.t('groupCall.startVideo'), onPressed: () => unawaited(startGroupCall(conv.id, video: true))),
          IconBtn('phone', color: t.muted, tooltip: context.t('groupCall.start'), onPressed: () => unawaited(startGroupCall(conv.id))),
        ],
        if (!conv.isGroup && conv.peerAccountId != null && !blocked) ...[
          IconBtn('video', color: t.muted, iconSize: 21, tooltip: context.t('call.startVideo'), onPressed: () => unawaited(startCall(conv.id, video: true))),
          IconBtn('phone', color: t.muted, tooltip: context.t('call.start'), onPressed: () => unawaited(startCall(conv.id))),
        ],
        IconBtn('more', color: t.muted, tooltip: context.t('common.more'), onPressed: () => showActionSheet(context, actions: _actions(context, blocked))),
      ]),
    );
  }
}

class _SelectionBar extends ConsumerWidget {
  const _SelectionBar({required this.conversationId});
  final String conversationId;

  List<Message> _selected() {
    final s = appData;
    return (s.selection?.ids ?? const <String>[]).map((id) => s.messages[id]).whereType<Message>().toList()
      ..sort((a, b) => a.receivedAt.compareTo(b.receivedAt));
  }

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final t = context.tk;
    final count = ref.watch(storeProvider.select((s) => s.selection?.ids.length ?? 0));
    Future<void> copy() async {
      final list = _selected().where((m) => !m.deleted && (m.text?.isNotEmpty ?? false)).toList();
      final lang = context.lang;
      final text = list.length == 1
          ? list.first.text!
          : list.map((m) => '[${formatTime(m.sentAt, lang)}] ${m.outgoing ? tr('common.you') : m.senderName}: ${m.text}').join('\n');
      await Clipboard.setData(ClipboardData(text: text));
      notifyOk(tr('common.copied'));
      store.set((d) => d.copyWith(selection: null));
    }

    return Container(
      height: 66,
      padding: const EdgeInsets.only(left: 6, right: 12),
      decoration: BoxDecoration(color: t.pane, border: Border(bottom: BorderSide(color: t.line))),
      child: Row(children: [
        IconBtn('x', iconSize: 20, tooltip: context.t('common.cancel'), onPressed: () => store.set((d) => d.copyWith(selection: null))),
        const SizedBox(width: 6),
        Expanded(child: Text(context.t('message.selected', count: count), style: ts(16, weight: 700, color: t.ink))),
        IconBtn('copy', color: t.muted, tooltip: context.t('message.copy'), onPressed: count == 0 ? null : copy),
        IconBtn('forward', color: t.muted, tooltip: context.t('message.forward'), onPressed: count == 0
            ? null
            // View-once media and media erased by the retention period are never forwarded.
            : () {
                final ids = _selected()
                    .where((m) => !m.deleted && !m.isViewOnce && !m.attachments.any((a) => a.isExpired))
                    .map((m) => m.id)
                    .toList();
                if (ids.isNotEmpty) showForwardSheet(context, ids);
              }),
        IconBtn('trash', color: t.danger, tooltip: context.t('message.delete'), onPressed: count == 0
            ? null
            : () => showDeleteDialog(context, conversationId, _selected().map((m) => m.id).toList())),
      ]),
    );
  }
}

/// In-chat search field (replaces the header while searching).
final _chatQuery = ValueNotifier<String>('');

class _ChatSearchBar extends StatefulWidget {
  const _ChatSearchBar({required this.onClose});
  final VoidCallback onClose;

  @override
  State<_ChatSearchBar> createState() => _ChatSearchBarState();
}

class _ChatSearchBarState extends State<_ChatSearchBar> {
  final _c = TextEditingController();

  @override
  void initState() {
    super.initState();
    _chatQuery.value = '';
  }

  @override
  void dispose() {
    _c.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    return Container(
      height: 66,
      padding: const EdgeInsets.only(left: 6, right: 12),
      decoration: BoxDecoration(color: t.pane, border: Border(bottom: BorderSide(color: t.line))),
      child: Row(children: [
        IconBtn('back', iconSize: 21, tooltip: context.t('common.back'), onPressed: widget.onClose),
        const SizedBox(width: 6),
        Expanded(
          child: SearchBox(controller: _c, hint: context.t('chat.searchIn'), autofocus: true, onChanged: (v) {
            setState(() {});
            _chatQuery.value = v.trim();
          }),
        ),
      ]),
    );
  }
}

class _ChatSearchResults extends StatefulWidget {
  const _ChatSearchResults({required this.conv, required this.onPick});
  final Conversation conv;
  final VoidCallback onPick;

  @override
  State<_ChatSearchResults> createState() => _ChatSearchResultsState();
}

class _ChatSearchResultsState extends State<_ChatSearchResults> {
  Timer? _debounce;
  List<Message>? _results;
  String _q = '';

  @override
  void initState() {
    super.initState();
    _chatQuery.addListener(_schedule);
  }

  @override
  void dispose() {
    _chatQuery.removeListener(_schedule);
    _debounce?.cancel();
    super.dispose();
  }

  void _schedule() {
    _debounce?.cancel();
    _debounce = Timer(const Duration(milliseconds: 250), () async {
      final q = _chatQuery.value;
      if (q.length < 2) {
        if (mounted) setState(() => _results = null);
        return;
      }
      try {
        final r = await backend.searchMessages(q, conversationId: widget.conv.id, limit: 40);
        if (mounted) {
          setState(() {
            _results = r;
            _q = q;
          });
        }
      } catch (_) {
        if (mounted) setState(() => _results = const []);
      }
    });
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    final r = _results;
    if (r == null) return const SizedBox.shrink();
    final lang = context.lang;
    return Container(
      color: t.pane,
      child: r.isEmpty
          ? Padding(padding: const EdgeInsets.all(24), child: Text(context.t('chats.noResults'), textAlign: TextAlign.center, style: ts(14, color: t.muted)))
          : ListView.separated(
              itemCount: r.length,
              separatorBuilder: (_, _) => Divider(height: 1, color: t.line),
              itemBuilder: (_, i) {
                final m = r[i];
                final text = previewText(m.kind, m.text, lang);
                final idx = text.toLowerCase().indexOf(_q.toLowerCase());
                return Tap(
                  onTap: () {
                    store.set((d) => d.copyWith(highlightId: m.id));
                    widget.onPick();
                  },
                  padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 10),
                  child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
                    Text('${m.outgoing ? context.t('common.you') : m.senderName} · ${formatDateTime(m.sentAt, lang)}', style: ts(12, color: t.muted)),
                    const SizedBox(height: 2),
                    Text.rich(
                      idx < 0
                          ? TextSpan(text: text)
                          : TextSpan(children: [
                              TextSpan(text: text.substring(0, idx)),
                              TextSpan(text: text.substring(idx, idx + _q.length), style: ts(14, weight: 700, color: t.accentText)),
                              TextSpan(text: text.substring(idx + _q.length)),
                            ]),
                      maxLines: 2,
                      overflow: TextOverflow.ellipsis,
                      style: ts(14, color: t.ink),
                    ),
                  ]),
                );
              },
            ),
    );
  }
}
