// Group invite links: the admin's link page, the join requests page, the requests banner and
// joining with a link. Port of app/src/features/groups/InviteDialogs.tsx.

import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../app/globals.dart';
import '../../app/nav.dart';
import '../../core/models.dart';
import '../../l10n/i18n.dart';
import '../../state/actions.dart';
import '../../state/store.dart';
import '../../theme/app_theme.dart';
import '../../util/format.dart';
import '../../widgets/app_icon.dart';
import '../../widgets/dialogs.dart';
import '../../widgets/ui.dart';

/// An invite link inside some text.
final inviteLinkRe = RegExp(r'chatapp://join/[A-Za-z0-9_-]{22}');

void _push(BuildContext context, Widget page) => Navigator.of(context).push(MaterialPageRoute<void>(builder: (_) => page));

// ---------------------------------------------------------------------------
// admins: the link
// ---------------------------------------------------------------------------

void openInviteLink(BuildContext context, String conversationId) => _push(context, InviteLinkPage(conversationId: conversationId));

class InviteLinkPage extends ConsumerStatefulWidget {
  const InviteLinkPage({super.key, required this.conversationId});
  final String conversationId;

  @override
  ConsumerState<InviteLinkPage> createState() => _InviteLinkPageState();
}

class _InviteLinkPageState extends ConsumerState<InviteLinkPage> {
  bool _approval = false;
  bool _busy = false;

  Future<void> _set({required bool enabled, bool? approval, bool reset = false}) async {
    setState(() => _busy = true);
    final g = await attempt(() => backend.setGroupInvite(widget.conversationId, enabled: enabled, approval: approval, reset: reset));
    if (g != null) store.upsertGroup(g);
    if (mounted) setState(() => _busy = false);
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    final group = ref.watch(storeProvider.select((s) => s.groups[widget.conversationId]));
    final link = group?.inviteLink;
    return PaneScaffold(
      header: PaneHeader(title: context.t('invite.title')),
      body: ListView(padding: const EdgeInsets.all(16), children: [
        if (link == null) ...[
          Text(context.t('invite.hint'), style: ts(14, color: t.muted, height: 1.45)),
          const SizedBox(height: 12),
          ListCard(children: [
            ToggleRow(
              label: context.t('invite.approval'),
              sub: context.t('invite.approvalHint'),
              on: _approval,
              onToggle: () => setState(() => _approval = !_approval),
            ),
          ]),
          const SizedBox(height: 16),
          Btn(label: context.t('invite.create'), icon: 'plus', busy: _busy, onPressed: () => _set(enabled: true, approval: _approval)),
        ] else ...[
          Container(
            padding: const EdgeInsets.all(14),
            decoration: BoxDecoration(color: t.sunk, borderRadius: BorderRadius.circular(12)),
            child: SelectableText(link, key: const ValueKey('invite-link'), style: ts(13, color: t.ink, mono: true)),
          ),
          const SizedBox(height: 10),
          Btn(
            label: context.t('common.copy'),
            icon: 'copy',
            kind: BtnKind.secondary,
            onPressed: () async {
              await Clipboard.setData(ClipboardData(text: link));
              notifyOk(tr('common.copied'));
            },
          ),
          const SizedBox(height: 8),
          Text(context.t('invite.shareHint'), style: ts(13, color: t.muted, height: 1.4)),
          const SizedBox(height: 14),
          ListCard(children: [
            ToggleRow(
              label: context.t('invite.approval'),
              sub: context.t('invite.approvalHint'),
              on: group!.inviteApproval,
              onToggle: _busy ? null : () => _set(enabled: true, approval: !group.inviteApproval),
            ),
            ListRow(
              icon: 'refresh',
              title: context.t('invite.reset'),
              onTap: _busy
                  ? null
                  : () async {
                      final ok = await showConfirm(
                        context,
                        title: context.t('invite.resetTitle'),
                        body: context.t('invite.resetBody'),
                        confirmLabel: context.t('invite.reset'),
                      );
                      if (ok) await _set(enabled: true, reset: true);
                    },
            ),
            ListRow(icon: 'x', title: context.t('invite.turnOff'), titleColor: t.danger, onTap: _busy ? null : () => _set(enabled: false)),
          ]),
        ],
      ]),
    );
  }
}

// ---------------------------------------------------------------------------
// admins: requests
// ---------------------------------------------------------------------------

void openJoinRequests(BuildContext context, String conversationId) => _push(context, JoinRequestsPage(conversationId: conversationId));

class JoinRequestsPage extends ConsumerStatefulWidget {
  const JoinRequestsPage({super.key, required this.conversationId});
  final String conversationId;

  @override
  ConsumerState<JoinRequestsPage> createState() => _JoinRequestsPageState();
}

class _JoinRequestsPageState extends ConsumerState<JoinRequestsPage> {
  List<JoinRequest>? _list;
  String? _busy;
  int? _loadedFor;

  Future<void> _load(int count) async {
    _loadedFor = count;
    try {
      final l = await backend.groupJoinRequests(widget.conversationId);
      if (mounted) setState(() => _list = l);
    } catch (e) {
      notifyError(e);
      if (mounted) setState(() => _list = const []);
    }
  }

  Future<void> _decide(JoinRequest r, bool approve) async {
    setState(() => _busy = r.accountId);
    final g = await attempt(() => approve
        ? backend.approveJoinRequest(widget.conversationId, r.accountId)
        : backend.rejectJoinRequest(widget.conversationId, r.accountId));
    if (g != null) {
      store.upsertGroup(g);
      _list = _list?.where((x) => x.accountId != r.accountId).toList();
    }
    if (mounted) setState(() => _busy = null);
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    final count = ref.watch(storeProvider.select((s) => s.groups[widget.conversationId]?.joinRequests ?? 0));
    if (_loadedFor != count) unawaited(_load(count));
    final list = _list;
    return PaneScaffold(
      header: PaneHeader(title: context.t('invite.requests')),
      body: list == null
          ? const Center(child: Spinner())
          : list.isEmpty
              ? Padding(
                  padding: const EdgeInsets.all(32),
                  child: Text(context.t('invite.noRequests'), textAlign: TextAlign.center, style: ts(14, color: t.muted)),
                )
              : ListView(children: [
                  for (final r in list)
                    Padding(
                      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 8),
                      child: Row(children: [
                        Avatar(name: r.name, path: r.avatarPath, seed: r.accountId, size: 44),
                        const SizedBox(width: 12),
                        Expanded(
                          child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
                            Text(r.name, maxLines: 1, overflow: TextOverflow.ellipsis, style: ts(15, weight: 600, color: t.ink)),
                            Text(context.t('invite.requestedAt', args: {'when': formatListTime(r.requestedAt, context.lang)}),
                                style: ts(12.5, color: t.muted)),
                          ]),
                        ),
                        IconBtn('x', color: t.muted, tooltip: context.t('invite.reject'), onPressed: _busy == r.accountId ? null : () => _decide(r, false)),
                        const SizedBox(width: 4),
                        Btn(
                          label: context.t('invite.approve'),
                          icon: 'check',
                          height: 38,
                          expand: false,
                          busy: _busy == r.accountId,
                          onPressed: () => _decide(r, true),
                        ),
                      ]),
                    ),
                ]),
    );
  }
}

// ---------------------------------------------------------------------------
// admins: removed (banned) members
// ---------------------------------------------------------------------------

void openGroupBans(BuildContext context, String conversationId) => _push(context, GroupBansPage(conversationId: conversationId));

class GroupBansPage extends StatefulWidget {
  const GroupBansPage({super.key, required this.conversationId});
  final String conversationId;

  @override
  State<GroupBansPage> createState() => _GroupBansPageState();
}

class _GroupBansPageState extends State<GroupBansPage> {
  List<GroupBan>? _list;
  String? _busy;

  @override
  void initState() {
    super.initState();
    unawaited(_load());
  }

  Future<void> _load() async {
    try {
      final l = await backend.groupBans(widget.conversationId);
      if (mounted) setState(() => _list = l);
    } catch (e) {
      notifyError(e);
      if (mounted) setState(() => _list = const []);
    }
  }

  Future<void> _unban(GroupBan b) async {
    setState(() => _busy = b.accountId);
    final g = await attempt(() => backend.unbanGroupMember(widget.conversationId, b.accountId));
    if (g != null) {
      store.upsertGroup(g);
      _list = _list?.where((x) => x.accountId != b.accountId).toList();
      notifyOk(tr('invite.unbanned', args: {'name': b.displayName}));
    }
    if (mounted) setState(() => _busy = null);
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    final list = _list;
    return PaneScaffold(
      header: PaneHeader(title: context.t('invite.bannedTitle')),
      body: list == null
          ? const Center(child: Spinner())
          : ListView(children: [
              Padding(
                padding: const EdgeInsets.fromLTRB(16, 16, 16, 8),
                child: Text(context.t('invite.bannedHint'), style: ts(13.5, color: t.muted, height: 1.4)),
              ),
              if (list.isEmpty)
                Padding(
                  padding: const EdgeInsets.all(32),
                  child: Text(context.t('invite.noBanned'), textAlign: TextAlign.center, style: ts(14, color: t.muted)),
                ),
              for (final b in list)
                Padding(
                  padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 8),
                  child: Row(children: [
                    Avatar(name: b.displayName, path: b.avatarPath, seed: b.accountId, size: 44),
                    const SizedBox(width: 12),
                    Expanded(
                      child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
                        Text(b.displayName, maxLines: 1, overflow: TextOverflow.ellipsis, style: ts(15, weight: 600, color: t.ink)),
                        Text(context.t('invite.bannedAt', args: {'when': formatListTime(b.bannedAt, context.lang)}),
                            style: ts(12.5, color: t.muted)),
                      ]),
                    ),
                    const SizedBox(width: 8),
                    Btn(
                      label: context.t('invite.unban'),
                      kind: BtnKind.secondary,
                      height: 38,
                      expand: false,
                      busy: _busy == b.accountId,
                      onPressed: () => _unban(b),
                    ),
                  ]),
                ),
            ]),
    );
  }
}

/// Admins: "N people asked to join" above the messages.
class JoinRequestsBanner extends ConsumerWidget {
  const JoinRequestsBanner({super.key, required this.conversationId});
  final String conversationId;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final t = context.tk;
    final count = ref.watch(storeProvider.select((s) {
      final g = s.groups[conversationId];
      return g?.myRole == 'admin' ? g!.joinRequests : 0;
    }));
    if (count == 0) return const SizedBox.shrink();
    return Tap(
      key: const ValueKey('join-banner'),
      onTap: () => openJoinRequests(context, conversationId),
      color: t.accentSoft,
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 10),
      child: Row(children: [
        AppIcon('users', size: 17, color: t.accentText),
        const SizedBox(width: 10),
        Expanded(child: Text(context.t('invite.banner', count: count), style: ts(14, color: t.ink))),
        Text(context.t('invite.review'), style: ts(14, weight: 650, color: t.accentText)),
      ]),
    );
  }
}

// ---------------------------------------------------------------------------
// joining with a link
// ---------------------------------------------------------------------------

void openJoinLink(BuildContext context, [String initial = '']) => _push(context, JoinLinkPage(initial: initial));

class JoinLinkPage extends StatefulWidget {
  const JoinLinkPage({super.key, this.initial = ''});
  final String initial;

  @override
  State<JoinLinkPage> createState() => _JoinLinkPageState();
}

class _JoinLinkPageState extends State<JoinLinkPage> {
  late final _text = TextEditingController(text: widget.initial);
  InvitePreview? _preview;
  String? _error;
  String? _previewFor;
  bool _busy = false;
  bool _requested = false;

  String? get _link => inviteLinkRe.firstMatch(_text.text)?.group(0);

  @override
  void initState() {
    super.initState();
    _text.addListener(_check);
    _check();
  }

  @override
  void dispose() {
    _text.dispose();
    super.dispose();
  }

  Future<void> _check() async {
    final link = _link;
    if (link == _previewFor) return;
    _previewFor = link;
    setState(() {
      _preview = null;
      _error = null;
    });
    if (link == null) return;
    try {
      final p = await backend.previewGroupInvite(link);
      if (!mounted || _previewFor != link) return;
      setState(() {
        _preview = p;
        _requested = p.requested;
      });
    } catch (e) {
      if (mounted && _previewFor == link) setState(() => _error = errorText(e));
    }
  }

  void _open(String? conversationId) {
    if (conversationId == null) {
      Navigator.of(context).maybePop();
      return;
    }
    openChat(conversationId);
  }

  Future<void> _join() async {
    final link = _link;
    if (link == null) return;
    setState(() => _busy = true);
    try {
      final r = await backend.joinGroupByLink(link);
      if (r.status == 'requested') {
        setState(() => _requested = true);
      } else {
        await reloadConversations();
        _open(r.conversationId);
      }
    } catch (e) {
      if (mounted) setState(() => _error = errorText(e));
    }
    if (mounted) setState(() => _busy = false);
  }

  Future<void> _cancel() async {
    final p = _preview;
    if (p == null) return;
    setState(() => _busy = true);
    await attempt(() => backend.cancelJoinRequest(p.groupId));
    if (mounted) setState(() => (_busy = false, _requested = false));
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    final p = _link == null ? null : _preview;
    return PaneScaffold(
      header: PaneHeader(title: context.t('invite.joinTitle')),
      body: ListView(padding: const EdgeInsets.all(16), children: [
        AppField(
          controller: _text,
          label: context.t('invite.linkLabel'),
          hint: 'chatapp://join/…',
          mono: true,
          autofocus: widget.initial.isEmpty,
          error: _text.text.isNotEmpty && _link == null ? context.t('invite.notALink') : _error,
        ),
        if (p != null) ...[
          const SizedBox(height: 20),
          Center(child: Avatar(name: p.name, seed: p.groupId, size: 72, group: true)),
          const SizedBox(height: 10),
          Text(p.name, key: const ValueKey('invite-name'), textAlign: TextAlign.center, style: ts(19, weight: 700, color: t.ink)),
          Text(context.t('group.subtitle', count: p.memberCount), textAlign: TextAlign.center, style: ts(13.5, color: t.muted)),
          if (p.description.isNotEmpty) ...[
            const SizedBox(height: 8),
            Text(p.description, textAlign: TextAlign.center, style: ts(14.5, color: t.ink, height: 1.4)),
          ],
          if (p.approval && !p.isMember) ...[
            const SizedBox(height: 8),
            Text(context.t('invite.needsApproval'), textAlign: TextAlign.center, style: ts(13, color: t.muted)),
          ],
          if (p.banned && !p.isMember) ...[
            const SizedBox(height: 8),
            Text(context.t('errors.banned'),
                key: const ValueKey('invite-banned'), textAlign: TextAlign.center, style: ts(13.5, weight: 600, color: t.danger)),
          ],
          const SizedBox(height: 20),
          if (p.isMember)
            Btn(label: context.t('invite.open'), onPressed: () => _open(p.conversationId))
          else if (_requested) ...[
            Text(context.t('invite.requestSent'), textAlign: TextAlign.center, style: ts(14, weight: 600, color: t.accentText)),
            const SizedBox(height: 10),
            Btn(label: context.t('invite.cancelRequest'), kind: BtnKind.secondary, busy: _busy, onPressed: _cancel),
          ] else
            Btn(label: context.t(p.approval ? 'invite.askToJoin' : 'invite.join'), busy: _busy, onPressed: p.banned ? null : _join),
        ],
      ]),
    );
  }
}
