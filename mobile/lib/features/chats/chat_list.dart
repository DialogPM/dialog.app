// Chat list: header, search (chats + messages), filter chips, rows (long press = menu).

import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../app/globals.dart';
import '../../app/nav.dart';
import '../../core/models.dart';
import '../../l10n/i18n.dart';
import '../../state/dnd.dart';
import '../../state/prefs.dart';
import '../../state/store.dart';
import '../../theme/app_theme.dart';
import '../../util/format.dart';
import '../../widgets/app_icon.dart';
import '../../widgets/dialogs.dart';
import '../../state/actions.dart';
import '../../widgets/ui.dart';
import 'chat_actions.dart';
import 'starred_page.dart';
import '../conversation/view_once_bubble.dart' show viewOnceLabel;
import '../groups/invite_pages.dart';

List<Conversation> sortConversations(Iterable<Conversation> list) => list.toList()
  ..sort((a, b) {
    if (a.pinned != b.pinned) return a.pinned ? -1 : 1;
    return b.sortTime.compareTo(a.sortTime);
  });

class ChatsPane extends ConsumerStatefulWidget {
  const ChatsPane({super.key});

  @override
  ConsumerState<ChatsPane> createState() => _ChatsPaneState();
}

class _ChatsPaneState extends ConsumerState<ChatsPane> {
  final _search = TextEditingController();
  String get _q => _search.text.trim().toLowerCase();

  @override
  void dispose() {
    _search.dispose();
    super.dispose();
  }

  void _more() {
    showActionSheet(context, actions: [
      SheetAction(
        label: context.t(dndActive() ? 'dnd.turnOff' : 'dnd.menu'),
        icon: 'moon',
        onTap: () => unawaited(toggleDnd(context)),
      ),
      SheetAction(label: context.t('newGroup.title'), icon: 'users', onTap: openNewGroup),
      SheetAction(label: context.t('qr.myCode'), icon: 'qr', onTap: () => openQr()),
      SheetAction(label: context.t('qr.scan'), icon: 'scan', onTap: () => openQr(tab: 'scan')),
      SheetAction(label: context.t('invite.joinTitle'), icon: 'link', onTap: () => openJoinLink(context)),
      SheetAction(label: context.t('star.title'), icon: 'star', onTap: () => openStarred(context)),
      SheetAction(label: context.t('chats.filter.archived'), icon: 'archive', onTap: () => store.set((d) => d.copyWith(chatFilter: 'archived'))),
      SheetAction(label: context.t('settings.title'), icon: 'gear', onTap: () => openSettingsSection('account')),
    ]);
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    final conversations = ref.watch(storeProvider.select((s) => s.conversations));
    final filter = ref.watch(storeProvider.select((s) => s.chatFilter));
    final dndUntil = ref.watch(prefsProvider.select((p) => p.dndUntil));
    final q = _q;
    final all = conversations.values;
    final archivedCount = all.where((c) => c.archived).length;
    final unreadCount = all.where((c) => !c.archived && c.unreadCount > 0).length;
    final requestCount = all.where((c) => c.isRequest).length;
    Iterable<Conversation> out;
    if (q.isNotEmpty) {
      final digits = q.replaceAll(' ', '');
      out = all.where((c) => c.title.toLowerCase().contains(q) || (c.peerNumber?.contains(digits) ?? false));
    } else if (filter == 'archived') {
      out = all.where((c) => c.archived);
    } else if (filter == 'unread') {
      out = all.where((c) => !c.archived && c.unreadCount > 0);
    } else if (filter == 'groups') {
      out = all.where((c) => !c.archived && c.isGroup);
    } else if (filter == 'requests') {
      out = all.where((c) => c.isRequest);
    } else {
      out = all.where((c) => !c.archived);
    }
    final list = sortConversations(out);

    Widget chip(String key, [int count = 0]) {
      final active = filter == key;
      return Padding(
        padding: const EdgeInsets.only(right: 6),
        child: Tap(
          onTap: () => store.set((d) => d.copyWith(chatFilter: key)),
          color: active ? t.accentSoft : t.sunk,
          radius: t.chipRadius(32),
          child: Container(
            height: 32,
            padding: const EdgeInsets.symmetric(horizontal: 13),
            alignment: Alignment.center,
            child: Row(mainAxisSize: MainAxisSize.min, children: [
              Text(context.t('chats.filter.$key'), style: ts(13.5, weight: 600, color: active ? t.accentText : t.muted)),
              if (count > 0) ...[
                const SizedBox(width: 6),
                Text('$count', style: ts(11.5, weight: 600, color: (active ? t.accentText : t.muted).withValues(alpha: .75))),
              ],
            ]),
          ),
        ),
      );
    }

    return Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
      Padding(
        padding: const EdgeInsets.fromLTRB(20, 18, 16, 12),
        child: Row(children: [
          Expanded(child: Text(context.t('chats.title'), style: ts(25, weight: 750, color: t.ink, letterSpacing: -0.6))),
          if (dndUntil > 0) ...[
            Tooltip(
              message: '${context.t('dnd.on')} · ${dndUntilText(dndUntil, context.lang)}',
              child: Tap(
                onTap: () => unawaited(toggleDnd(context)),
                color: t.accentSoft,
                radius: BorderRadius.circular(999),
                child: Padding(
                  padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 5),
                  child: Row(mainAxisSize: MainAxisSize.min, children: [
                    AppIcon('moon', size: 14, color: t.accentText),
                    const SizedBox(width: 5),
                    Text(context.t('dnd.badge'), style: ts(12, weight: 650, color: t.accentText)),
                  ]),
                ),
              ),
            ),
            const SizedBox(width: 6),
          ],
          IconBtn('edit', size: 38, iconSize: 19, tooltip: context.t('newChat.title'), onPressed: openNewChat),
          const SizedBox(width: 4),
          IconBtn('more', size: 38, iconSize: 19, tooltip: context.t('common.more'), onPressed: _more),
        ]),
      ),
      Padding(
        padding: const EdgeInsets.symmetric(horizontal: 16),
        child: SearchBox(controller: _search, hint: context.t('chats.search'), onChanged: (_) => setState(() {})),
      ),
      if (q.isEmpty)
        SizedBox(
          height: 56,
          child: ListView(
            scrollDirection: Axis.horizontal,
            padding: const EdgeInsets.fromLTRB(16, 12, 10, 12),
            children: [
              chip('all'),
              // Message requests get their own filter while there are any (they also stay in "all").
              if (requestCount > 0 || filter == 'requests') chip('requests', requestCount),
              chip('unread', unreadCount),
              chip('groups'),
              chip('archived', archivedCount),
            ],
          ),
        )
      else
        const SizedBox(height: 8),
      Expanded(
        child: q.isNotEmpty
            ? _SearchResults(query: q, conversations: list)
            : list.isEmpty
                ? _EmptyList(filter: filter)
                : ListView.builder(
                    padding: const EdgeInsets.only(bottom: 8),
                    itemCount: list.length + 1,
                    itemBuilder: (context, i) => i == list.length
                        ? const LockNote(padding: EdgeInsets.fromLTRB(20, 18, 20, 8))
                        : ChatListItem(key: ValueKey(list[i].id), conv: list[i]),
                  ),
      ),
    ]);
  }
}

class SearchBox extends StatelessWidget {
  const SearchBox({super.key, required this.controller, required this.hint, this.onChanged, this.autofocus = false});

  final TextEditingController controller;
  final String hint;
  final ValueChanged<String>? onChanged;
  final bool autofocus;

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    return Container(
      height: 42,
      padding: const EdgeInsets.only(left: 14, right: 6),
      decoration: BoxDecoration(color: t.sunk, borderRadius: t.inputRadius(42)),
      child: Row(children: [
        AppIcon('search', size: 17, color: t.muted),
        const SizedBox(width: 10),
        Expanded(
          child: TextField(
            enableIMEPersonalizedLearning: imePersonalizedLearning,
            controller: controller,
            autofocus: autofocus,
            onChanged: onChanged,
            style: ts(14.5, color: t.ink),
            decoration: InputDecoration(
              isDense: true,
              border: InputBorder.none,
              hintText: hint,
              hintStyle: ts(14.5, color: t.ink.withValues(alpha: .5)),
              contentPadding: EdgeInsets.zero,
            ),
          ),
        ),
        if (controller.text.isNotEmpty)
          IconBtn('x', size: 28, iconSize: 15, round: true, color: t.muted, tooltip: context.t('common.clear'), onPressed: () {
            controller.clear();
            onChanged?.call('');
          }),
      ]),
    );
  }
}

class LockNote extends StatelessWidget {
  const LockNote({super.key, this.padding = EdgeInsets.zero, this.text});
  final EdgeInsetsGeometry padding;
  final String? text;

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    return Padding(
      padding: padding,
      child: Row(mainAxisAlignment: MainAxisAlignment.center, children: [
        AppIcon('lock', size: 13, color: t.faint),
        const SizedBox(width: 6),
        Flexible(child: Text(text ?? context.t('chats.e2eFooter'), textAlign: TextAlign.center, style: ts(12, color: t.faint))),
      ]),
    );
  }
}

class _EmptyList extends StatelessWidget {
  const _EmptyList({required this.filter});
  final String filter;

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    return Padding(
      padding: const EdgeInsets.fromLTRB(24, 40, 24, 24),
      child: Column(children: [
        Text(context.t('chats.empty.$filter'), textAlign: TextAlign.center, style: ts(14, color: t.muted)),
        if (filter != 'all') ...[
          const SizedBox(height: 10),
          LinkBtn(label: context.t('chats.showAll'), onPressed: () => store.set((d) => d.copyWith(chatFilter: 'all'))),
        ],
      ]),
    );
  }
}

/// Status ticks of an outgoing message: clock, one check, two checks.
class StatusTicks extends StatelessWidget {
  const StatusTicks({super.key, required this.status, this.size = 15, required this.color, required this.readColor});

  final String status;
  final double size;
  final Color color;
  final Color readColor;

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    if (status == 'failed') return AppIcon('alert', size: size, color: t.danger, semanticLabel: context.t('status.failed'));
    final name = status == 'pending' ? 'clock' : status == 'sent' ? 'check' : 'checks';
    return AppIcon(name,
        size: status == 'pending' ? size - 3 : size, color: status == 'read' ? readColor : color, semanticLabel: context.t('status.$status'));
  }
}

String? typingText(BuildContext context, Conversation conv, Map<String, TypingEntry>? typing) {
  final names = typing?.values.map((x) => x.name).toList() ?? const [];
  if (names.isEmpty) return null;
  if (!conv.isGroup) return context.t('presence.typing');
  if (names.length == 1) return context.t('presence.typingName', args: {'name': names.first});
  return context.t('presence.typingMany', count: names.length);
}

class ChatListItem extends ConsumerWidget {
  const ChatListItem({super.key, required this.conv, this.onTap, this.showPreviewOf});

  final Conversation conv;
  final VoidCallback? onTap;

  /// Search result: show this message instead of the last one.
  final (Message, String)? showPreviewOf;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final t = context.tk;
    final lang = context.lang;
    final verified = ref.watch(storeProvider.select((s) => conv.peerAccountId == null ? false : (s.contacts[conv.peerAccountId]?.verified ?? false)));
    final typing = typingText(context, conv, ref.watch(storeProvider.select((s) => s.typing[conv.id])));
    final muted = conv.isMuted();
    final last = conv.lastMessage;
    final unread = conv.unreadCount;

    Widget? lead;
    InlineSpan preview;
    int? time = last?.sentAt;
    final sp = showPreviewOf;
    if (sp != null) {
      final m = sp.$1;
      time = m.sentAt;
      final prefix = m.outgoing ? '${context.t('common.you')}: ' : conv.isGroup ? '${isolate(m.senderName)}: ' : '';
      final text = m.isViewOnce
          ? viewOnceLabel(m.kind, lang)
          : previewText(m.kind, m.text, lang, mediaExpired: m.attachments.any((a) => a.isExpired));
      preview = TextSpan(children: [TextSpan(text: prefix), ..._highlight(text, sp.$2, t.ink)]);
    } else if (typing != null) {
      preview = TextSpan(text: typing, style: ts(14, weight: 600, color: t.accentText));
    } else if (conv.draft != null && conv.draft!.isNotEmpty) {
      preview = TextSpan(children: [
        TextSpan(text: '${context.t('chat.draft')} ', style: ts(14, weight: 600, color: t.danger)),
        TextSpan(text: conv.draft),
      ]);
    } else if (last != null) {
      final text = last.deleted
          ? context.t(last.outgoing ? 'message.deletedByYou' : 'message.deleted')
          : last.kind == 'system'
              ? (last.system != null && systemText(last.system!, lang).isNotEmpty
                  ? systemText(last.system!, lang)
                  : context.t('message.groupEvent'))
              : last.viewOnce
                  ? viewOnceLabel(last.kind, lang)
                  : previewText(last.kind, last.text, lang, mediaExpired: last.mediaExpired);
      final prefix = conv.isGroup && !last.deleted && last.kind != 'system' && !last.outgoing && (last.senderName?.isNotEmpty ?? false)
          ? '${last.senderName!.split(' ').first}: '
          : '';
      if (last.outgoing && !last.deleted && last.kind != 'system') {
        lead = Padding(
          padding: const EdgeInsets.only(right: 5),
          child: StatusTicks(status: last.status, size: 16, color: t.faint, readColor: t.accentText),
        );
      }
      preview = TextSpan(
        style: last.deleted ? const TextStyle(fontStyle: FontStyle.italic) : null,
        children: [
          if (prefix.isNotEmpty) TextSpan(text: prefix, style: ts(14, weight: 500, color: t.ink)),
          TextSpan(text: text),
        ],
      );
    } else {
      preview = const TextSpan(text: '');
    }

    return Padding(
      padding: EdgeInsets.symmetric(horizontal: t.rowMx),
      child: Tap(
        radius: BorderRadius.circular(t.rowRadius),
        onTap: onTap ?? () => openChat(conv.id),
        onLongPress: () => showActionSheet(context, title: conv.title, actions: listActions(context, conv)),
        padding: EdgeInsets.symmetric(horizontal: t.rowPx, vertical: 11),
        child: Row(children: [
          Stack(clipBehavior: Clip.none, children: [
            Avatar(name: conv.title, path: conv.avatarPath, seed: conv.id, size: 50, group: conv.isGroup),
            // Disappearing messages on (like other messengers): visible before opening the chat.
            if (conv.disappearingSeconds > 0)
              Positioned(
                right: -2,
                bottom: -2,
                child: Semantics(
                  label: context.t('disappearing.title'),
                  child: Container(
                    width: 20,
                    height: 20,
                    decoration: BoxDecoration(color: t.ink, shape: BoxShape.circle, border: Border.all(color: t.pane, width: 2)),
                    child: Center(child: AppIcon('timer', size: 12, color: t.pane)),
                  ),
                ),
              ),
          ]),
          const SizedBox(width: 13),
          Expanded(
            child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
              Row(children: [
                Expanded(
                  child: Row(children: [
                    Flexible(child: Text(conv.title, maxLines: 1, overflow: TextOverflow.ellipsis, style: ts(15.5, weight: 650, color: t.ink))),
                    if (verified) ...[const SizedBox(width: 6), AppIcon('shield', size: 14, color: t.accentText, semanticLabel: context.t('safety.verified'))],
                  ]),
                ),
                if (time != null) ...[
                  const SizedBox(width: 6),
                  Text(formatListTime(time, lang), style: ts(12, weight: unread > 0 ? 700 : 500, color: unread > 0 ? t.accentText : t.faint)),
                ],
              ]),
              const SizedBox(height: 3),
              Row(children: [
                ?lead,
                if (conv.isRequest)
                  Container(
                    margin: const EdgeInsets.only(right: 6),
                    padding: const EdgeInsets.symmetric(horizontal: 7, vertical: 1),
                    decoration: BoxDecoration(color: t.accentSoft, borderRadius: BorderRadius.circular(999)),
                    child: Text(context.t(conv.isGroup ? 'request.groupBadge' : 'request.badge'), style: ts(11.5, weight: 650, color: t.accentText)),
                  ),
                Expanded(
                  child: Text.rich(preview, maxLines: 1, overflow: TextOverflow.ellipsis, style: ts(14, color: t.muted)),
                ),
                if (conv.blocked) ...[const SizedBox(width: 5), AppIcon('ban', size: 15, color: t.faint)],
                if (muted) ...[const SizedBox(width: 5), AppIcon('mute', size: 15, color: t.faint, semanticLabel: context.t('chat.muted'))],
                if (conv.pinned && !conv.archived) ...[const SizedBox(width: 5), AppIcon('pin', size: 15, color: t.faint, semanticLabel: context.t('chat.pinned'))],
                if (unread > 0 && sp == null && conv.unreadMentions > 0) ...[
                  const SizedBox(width: 5),
                  Semantics(
                    label: context.t('mention.unread'),
                    child: Container(
                      width: 21,
                      height: 21,
                      decoration: BoxDecoration(color: t.accent, borderRadius: BorderRadius.circular(999)),
                      alignment: Alignment.center,
                      child: Text('@', style: ts(12, weight: 700, color: t.onAccent, height: 1)),
                    ),
                  ),
                ],
                if (unread > 0 && sp == null) ...[
                  const SizedBox(width: 5),
                  Container(
                    constraints: const BoxConstraints(minWidth: 21),
                    height: 21,
                    padding: const EdgeInsets.symmetric(horizontal: 6),
                    decoration: BoxDecoration(color: muted ? t.faint : t.accent, borderRadius: BorderRadius.circular(999)),
                    alignment: Alignment.center,
                    child: Text(unread > 99 ? '99+' : '$unread', style: ts(11.5, weight: 700, color: muted ? t.pane : t.onAccent, height: 1)),
                  ),
                ],
              ]),
            ]),
          ),
        ]),
      ),
    );
  }
}

List<InlineSpan> _highlight(String text, String query, Color ink) {
  final i = text.toLowerCase().indexOf(query.toLowerCase());
  if (i < 0 || query.isEmpty) return [TextSpan(text: text)];
  return [
    TextSpan(text: text.substring(0, i)),
    TextSpan(text: text.substring(i, i + query.length), style: TextStyle(color: ink, fontWeight: FontWeight.w700)),
    TextSpan(text: text.substring(i + query.length)),
  ];
}

class SectionLabel extends StatelessWidget {
  const SectionLabel(this.text, {super.key});
  final String text;

  @override
  Widget build(BuildContext context) => Padding(
        padding: const EdgeInsets.fromLTRB(20, 14, 20, 6),
        child: Text(text.toUpperCase(), style: ts(12, weight: 600, color: context.tk.muted, letterSpacing: .72)),
      );
}

class _SearchResults extends ConsumerStatefulWidget {
  const _SearchResults({required this.query, required this.conversations});
  final String query;
  final List<Conversation> conversations;

  @override
  ConsumerState<_SearchResults> createState() => _SearchResultsState();
}

class _SearchResultsState extends ConsumerState<_SearchResults> {
  Timer? _debounce;
  List<Message> _messages = const [];
  String _searched = '';
  FoundAccount? _found;

  /// A 16-digit account number or a @username: worth asking the directory.
  static bool _isAccountQuery(String q) {
    final digits = q.replaceAll(RegExp(r'[\s-]'), '');
    if (RegExp(r'^\d{16}$').hasMatch(digits)) return true;
    // Phone numbers: international (+40..., 0040...) or national (07..., with our own country).
    final phoneDigits = q.replaceAll(RegExp(r'\D'), '').length;
    if (RegExp(r'^\+?[\d\s().-]+$').hasMatch(q) && RegExp(r'^(\+|0)').hasMatch(q) && phoneDigits >= 6 && phoneDigits <= 15) {
      return true;
    }
    return RegExp(r'^@?[a-z][a-z0-9_.]{2,31}$').hasMatch(q);
  }

  @override
  void initState() {
    super.initState();
    _schedule();
  }

  @override
  void didUpdateWidget(_SearchResults old) {
    super.didUpdateWidget(old);
    if (old.query != widget.query) _schedule();
  }

  @override
  void dispose() {
    _debounce?.cancel();
    super.dispose();
  }

  void _schedule() {
    _debounce?.cancel();
    _debounce = Timer(const Duration(milliseconds: 250), () async {
      final q = widget.query;
      if (q.length < 2) {
        if (mounted) {
          setState(() {
            _messages = const [];
            _found = null;
          });
        }
        return;
      }
      if (_isAccountQuery(q)) {
        backend.findAccount(q).then((f) {
          if (mounted && widget.query == q) setState(() => _found = f);
        }).catchError((Object _) {
          if (mounted) setState(() => _found = null);
        });
      } else if (_found != null) {
        setState(() => _found = null);
      }
      try {
        final res = await backend.searchMessages(q, limit: 50);
        if (mounted && widget.query == q) {
          setState(() {
            _messages = res;
            _searched = q;
          });
        }
      } catch (_) {}
    });
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    final convs = ref.watch(storeProvider.select((s) => s.conversations));
    final msgs = _messages.where((m) => convs.containsKey(m.conversationId)).toList();
    final found = _found;
    return ListView(padding: const EdgeInsets.only(bottom: 8), children: [
      if (found != null) ...[
        SectionLabel(context.t('mobile.searchAccount')),
        _FoundAccountRow(found: found),
      ],
      if (widget.conversations.isNotEmpty) SectionLabel(context.t('chats.searchChats')),
      for (final c in widget.conversations) ChatListItem(conv: c),
      if (msgs.isNotEmpty) SectionLabel(context.t('chats.searchMessages')),
      for (final m in msgs)
        ChatListItem(
          conv: convs[m.conversationId]!,
          showPreviewOf: (m, _searched),
          onTap: () => openChat(m.conversationId, highlightId: m.id),
        ),
      if (widget.conversations.isEmpty && msgs.isEmpty && found == null)
        Padding(
          padding: const EdgeInsets.all(40),
          child: Text(context.t('chats.noResults'), textAlign: TextAlign.center, style: ts(14, color: t.muted)),
        ),
    ]);
  }
}

/// Account found in the directory by number or @username, with a "start chat" button.
class _FoundAccountRow extends StatefulWidget {
  const _FoundAccountRow({required this.found});
  final FoundAccount found;

  @override
  State<_FoundAccountRow> createState() => _FoundAccountRowState();
}

class _FoundAccountRowState extends State<_FoundAccountRow> {
  bool _busy = false;

  Future<void> _start() async {
    setState(() => _busy = true);
    final f = widget.found;
    try {
      Conversation c;
      try {
        c = await backend.openDirectConversation(f.accountId);
      } catch (_) {
        // Not a contact yet: add it from the directory first.
        final contact = await backend.addContactByQuery(f.username != null ? '@${f.username}' : f.number);
        store.upsertContact(contact);
        c = await backend.openDirectConversation(contact.accountId);
      }
      store.upsertConversation(c);
      openChat(c.id);
    } catch (e) {
      notifyError(e);
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    final f = widget.found;
    final sub = [if (f.username != null) '@${f.username}', formatAccountNumber(f.number)].join(' · ');
    return Padding(
      padding: EdgeInsets.symmetric(horizontal: t.rowMx),
      child: Padding(
        padding: EdgeInsets.symmetric(horizontal: t.rowPx, vertical: 10),
        child: Row(children: [
          Avatar(name: f.displayName, path: f.avatarPath, seed: f.accountId, size: 50),
          const SizedBox(width: 13),
          Expanded(
            child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
              Text(f.displayName, maxLines: 1, overflow: TextOverflow.ellipsis, style: ts(15.5, weight: 650, color: t.ink)),
              Text(f.isMe ? context.t('mobile.itsYou') : sub, maxLines: 1, overflow: TextOverflow.ellipsis, style: ts(13, color: t.muted)),
            ]),
          ),
          if (!f.isMe) ...[
            const SizedBox(width: 8),
            Btn(label: context.t('mobile.startChat'), icon: 'chat', height: 36, fontSize: 13, expand: false, busy: _busy, onPressed: _start),
          ],
        ]),
      ),
    );
  }
}
