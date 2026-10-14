// Contact / group info pane (full screen on phones), media gallery and safety number screens.

import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../services/media.dart';
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
import '../../widgets/qr.dart';
import '../../widgets/ui.dart';
import '../groups/invite_pages.dart';
import '../settings/key_transparency.dart';
import '../chats/chat_actions.dart';
import '../chats/contact_picker.dart';
import '../chats/new_group_page.dart';
import '../conversation/attachments.dart';
import '../conversation/conversation_page.dart';
import '../conversation/lightbox.dart';
import '../conversation/subtitle.dart';

class InfoPage extends ConsumerWidget {
  const InfoPage({super.key, required this.conversationId});
  final String conversationId;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final conv = ref.watch(storeProvider.select((s) => s.conversations[conversationId]));
    if (conv == null) return Scaffold(backgroundColor: context.tk.pane);
    return PaneScaffold(
      header: PaneHeader(
        titleWidget: Text(context.t(conv.isGroup ? 'group.info' : 'contact.info'), style: ts(16.5, weight: 700, color: context.tk.ink)),
      ),
      body: ListView(padding: const EdgeInsets.only(bottom: 24), children: [
        if (conv.isGroup) _GroupInfo(conv: conv) else _ContactInfo(conv: conv),
      ]),
    );
  }
}

// ---------------------------------------------------------------------------
// shared pieces
// ---------------------------------------------------------------------------

class _Action {
  const _Action(this.icon, this.label, this.onTap);
  final String icon;
  final String label;
  final VoidCallback onTap;
}

/// The 86x66 quick action tiles under the avatar.
class _InfoActions extends StatelessWidget {
  const _InfoActions({required this.actions});
  final List<_Action> actions;

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    return Padding(
      padding: const EdgeInsets.only(top: 18),
      child: Wrap(spacing: 8, alignment: WrapAlignment.center, children: [
        for (final a in actions)
          SizedBox(
            width: 86,
            height: 66,
            child: Tap(
              onTap: a.onTap,
              color: t.sunk,
              radius: BorderRadius.circular(16),
              child: Column(mainAxisAlignment: MainAxisAlignment.center, children: [
                AppIcon(a.icon, size: 19, color: t.accentText),
                const SizedBox(height: 6),
                Text(a.label, maxLines: 1, overflow: TextOverflow.ellipsis, style: ts(12.5, weight: 600, color: t.ink)),
              ]),
            ),
          ),
      ]),
    );
  }
}

void _searchInChat(BuildContext context, String conversationId) {
  Navigator.of(context).pop();
  chatSearchRequest.value = conversationId;
}

class _Hero extends StatelessWidget {
  const _Hero({required this.avatar, required this.name, this.nameTrailing = const [], required this.sub, this.subAccent = false, this.extra = const [], required this.actions});

  final Widget avatar;
  final String name;
  final List<Widget> nameTrailing;
  final String sub;
  final bool subAccent;
  final List<Widget> extra;
  final List<_Action> actions;

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    return Padding(
      padding: const EdgeInsets.fromLTRB(20, 28, 20, 22),
      child: Column(children: [
        avatar,
        const SizedBox(height: 16),
        Row(mainAxisAlignment: MainAxisAlignment.center, children: [
          Flexible(child: Text(name, maxLines: 1, overflow: TextOverflow.ellipsis, style: ts(22, weight: 750, color: t.ink, letterSpacing: -.33))),
          ...nameTrailing,
        ]),
        const SizedBox(height: 4),
        Text(sub, textAlign: TextAlign.center, style: ts(14, color: subAccent ? t.accentText : t.muted)),
        ...extra,
        _InfoActions(actions: actions),
      ]),
    );
  }
}

class _MediaSection extends ConsumerStatefulWidget {
  const _MediaSection({required this.conversationId});
  final String conversationId;

  @override
  ConsumerState<_MediaSection> createState() => _MediaSectionState();
}

class _MediaSectionState extends ConsumerState<_MediaSection> {
  List<Message>? _media;
  int _files = 0;
  String? _loadedFor;

  void _load(String? last) {
    if (_loadedFor == last && _media != null) return;
    _loadedFor = last;
    backend.listMedia(widget.conversationId, 'media', limit: 200).then((m) {
      if (mounted) setState(() => _media = m);
    }).catchError((Object _) {
      if (mounted) setState(() => _media = const []);
    });
    backend.listMedia(widget.conversationId, 'files', limit: 200).then((m) {
      if (mounted) setState(() => _files = m.fold(0, (n, x) => n + x.attachments.length));
    }).catchError((Object _) {});
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    final last = ref.watch(storeProvider.select((s) => s.conversations[widget.conversationId]?.lastMessage?.id));
    _load(last);
    final items = <(Message, Attachment, int)>[
      for (final m in _media ?? const <Message>[])
        for (final (i, a) in m.attachments.where((a) => a.isImage || a.isVideo).indexed) (m, a, i),
    ];
    final thumbs = items.take(3).toList();
    return Sect(
      child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
        GestureDetector(
          behavior: HitTestBehavior.opaque,
          onTap: () => Navigator.of(context).push(MaterialPageRoute<void>(builder: (_) => MediaGalleryPage(conversationId: widget.conversationId))),
          child: Row(children: [
            Expanded(child: Text(context.t('info.media'), style: ts(14.5, weight: 650, color: t.ink))),
            Text('${items.length + _files}', style: ts(13, color: t.muted)),
            const SizedBox(width: 4),
            AppIcon('chevron', size: 16, color: t.muted),
          ]),
        ),
        if (thumbs.isNotEmpty) ...[
          const SizedBox(height: 12),
          Row(children: [
            for (var k = 0; k < 3; k++) ...[
              if (k > 0) const SizedBox(width: 6),
              Expanded(
                child: AspectRatio(
                  aspectRatio: 1,
                  child: k < thumbs.length ? _Thumb(m: thumbs[k].$1, a: thumbs[k].$2, index: thumbs[k].$3) : const SizedBox(),
                ),
              ),
            ],
          ]),
        ],
      ]),
    );
  }
}

class _Thumb extends StatelessWidget {
  const _Thumb({required this.m, required this.a, required this.index});
  final Message m;
  final Attachment a;
  final int index;

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    final path = a.localPath;
    final thumb = thumbBytes(a.thumbnail);
    Widget img;
    if (path != null && a.isImage) {
      img = MediaImage(path, fit: BoxFit.cover, cacheWidth: 400);
    } else if (thumb != null) {
      img = ImageFiltered(imageFilter: ImageFilterBlur.sigma(4), child: SafeMemoryImage(thumb, fit: BoxFit.cover, cacheWidth: 400));
    } else {
      img = Container(color: t.sunk, child: a.isVideo ? Center(child: AppIcon('play', size: 24, color: t.muted)) : const ImgPlaceholder());
    }
    return GestureDetector(
      onTap: () {
        if (path != null && a.isImage) {
          openLightbox(context, m, m.attachments.where((x) => x.isImage).toList().indexOf(a));
        } else if (path != null) {
          openAttachment(m, a);
        } else {
          downloadAttachment(m, a);
        }
      },
      child: ClipRRect(borderRadius: BorderRadius.circular(10), child: SizedBox.expand(child: img)),
    );
  }
}

class _ChatToggles extends ConsumerWidget {
  const _ChatToggles({required this.conv});
  final Conversation conv;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final muted = conv.isMuted();
    // Group timer: a group rule only admins change.
    final timerLocked = ref.watch(storeProvider.select((s) => conv.isGroup && s.groups[conv.id]?.myRole != 'admin'));
    final timerLabel = disappearingLabel(conv.disappearingSeconds, context.lang);
    return Sect(
      padding: const EdgeInsets.symmetric(vertical: 4),
      child: Column(children: [
        ToggleRow(
          icon: 'bell',
          label: context.t('info.muteNotifications'),
          sub: context.t(muted ? 'info.on' : 'info.off'),
          on: muted,
          onToggle: () => toggleMute(context, conv),
        ),
        if (!conv.left)
          ToggleRow(
            icon: 'clock',
            label: context.t('disappearing.title'),
            sub: timerLocked ? '$timerLabel · ${context.t('groupRules.timerByAdmins')}' : timerLabel,
            on: conv.disappearingSeconds > 0,
            onToggle: timerLocked ? null : () => openDisappearingDialog(context, conv),
          ),
      ]),
    );
  }
}

class _DangerRow extends StatelessWidget {
  const _DangerRow({required this.icon, required this.label, required this.onTap, this.ink = false});
  final String icon;
  final String label;
  final VoidCallback onTap;
  final bool ink;

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    final c = ink ? t.ink : t.danger;
    return Tap(
      onTap: onTap,
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 13),
      child: Row(children: [
        AppIcon(icon, size: 19, color: c),
        const SizedBox(width: 14),
        Expanded(child: Text(label, style: ts(14.5, weight: 600, color: c))),
      ]),
    );
  }
}

// ---------------------------------------------------------------------------
// contact
// ---------------------------------------------------------------------------

Future<void> renameContactDialog(BuildContext context, Contact contact) async {
  var reset = false;
  final v = await showTextEdit(
    context,
    title: context.t('contact.rename'),
    label: context.t('contact.name'),
    description: contact.profileName != null ? context.t('contact.profileName', args: {'name': contact.profileName}) : null,
    initial: contact.localName ?? contact.displayName,
    maxLength: 60,
    allowEmpty: true,
    extraAction: contact.localName == null
        ? null
        : Builder(
            builder: (ctx) => LinkBtn(
              label: ctx.t('contact.resetName'),
              color: ctx.tk.muted,
              fontSize: 13,
              onPressed: () {
                reset = true;
                Navigator.of(ctx).pop();
              },
            ),
          ),
  );
  if (v == null && !reset) return;
  final name = reset || (v?.isEmpty ?? true) ? null : v;
  final c = await attempt(() => backend.renameContact(contact.accountId, name));
  if (c != null) {
    store.upsertContact(c);
    unawaited(reloadConversations());
  }
}

class _ContactInfo extends ConsumerWidget {
  const _ContactInfo({required this.conv});
  final Conversation conv;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final t = context.tk;
    final contact = ref.watch(storeProvider.select((s) => conv.peerAccountId == null ? null : s.contacts[conv.peerAccountId]));
    final sub = conversationSubtitle(context, ref, conv);
    final number = contact?.number ?? conv.peerNumber ?? '';
    final blocked = contact?.blocked ?? conv.blocked;
    final name = contact?.displayName ?? conv.title;
    final muted = conv.isMuted();
    return Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
      _Hero(
        avatar: Avatar(name: conv.title, path: contact?.avatarPath ?? conv.avatarPath, seed: conv.id, size: 112),
        name: name,
        nameTrailing: [
          if (contact?.verified ?? false) ...[const SizedBox(width: 7), AppIcon('shield', size: 18, color: t.accentText)],
          if (contact != null) ...[
            const SizedBox(width: 4),
            IconBtn('edit', size: 32, iconSize: 16, color: t.muted, tooltip: context.t('contact.rename'), onPressed: () => renameContactDialog(context, contact)),
          ],
        ],
        sub: sub.text,
        subAccent: sub.accent,
        extra: [
          if (number.isNotEmpty)
            Padding(
              padding: const EdgeInsets.only(top: 6),
              child: Row(mainAxisAlignment: MainAxisAlignment.center, children: [
                Text(formatAccountNumber(number), style: ts(13.5, color: t.muted, mono: true, letterSpacing: .5)),
                IconBtn('copy', size: 28, iconSize: 14, color: t.muted, tooltip: context.t('common.copy'), onPressed: () async {
                  await Clipboard.setData(ClipboardData(text: number));
                  notifyOk(tr('common.copied'));
                }),
              ]),
            ),
          if (contact?.profileName != null && contact?.localName != null) Text('~${contact!.profileName}', style: ts(12.5, color: t.muted)),
        ],
        actions: [
          _Action('search', context.t('info.search'), () => _searchInChat(context, conv.id)),
          _Action(muted ? 'bell' : 'mute', context.t(muted ? 'info.unmute' : 'info.mute'), () => toggleMute(context, conv)),
          if (conv.peerAccountId != null)
            _Action('qr', context.t('info.qr'), () => Navigator.of(context).push(MaterialPageRoute<void>(
                  builder: (_) => SafetyNumberPage(accountId: conv.peerAccountId!, name: name),
                ))),
        ],
      ),
      if ((contact?.about?.isNotEmpty ?? false) || (contact?.phone?.isNotEmpty ?? false))
        Sect(
          child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
            if (contact?.about?.isNotEmpty ?? false) ...[
              Text(context.t('contact.about'), style: ts(12.5, weight: 600, color: t.muted)),
              const SizedBox(height: 4),
              Text(contact!.about!, style: ts(14.5, color: t.ink, height: 1.45)),
            ],
            if (contact?.phone?.isNotEmpty ?? false) ...[
              if (contact?.about?.isNotEmpty ?? false) const SizedBox(height: 12),
              Row(children: [
                AppIcon('phone', size: 17, color: t.muted),
                const SizedBox(width: 10),
                Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
                  Text(contact!.phone!, style: ts(14.5, color: t.ink)),
                  Text(context.t('contact.fromAddressBook'), style: ts(12.5, color: t.muted)),
                ]),
              ]),
            ],
          ]),
        ),
      _MediaSection(conversationId: conv.id),
      _ChatToggles(conv: conv),
      if (conv.peerAccountId != null) _SafetySection(accountId: conv.peerAccountId!, name: name),
      Sect(
        padding: const EdgeInsets.symmetric(vertical: 4),
        child: Column(children: [
          if (conv.peerAccountId != null)
            _DangerRow(
              icon: 'alert',
              label: blocked ? context.t('chat.block.unblockName', args: {'name': conv.title}) : context.t('chat.block.blockContact'),
              ink: blocked,
              onTap: () => confirmBlock(context, conv.peerAccountId!, conv.title, !blocked),
            ),
          _DangerRow(
            icon: 'trash',
            label: context.t('chat.delete.action'),
            onTap: () async {
              final nav = Navigator.of(context);
              if (await confirmDeleteChat(context, conv)) nav.popUntil((r) => r.isFirst);
            },
          ),
        ]),
      ),
    ]);
  }
}

/// Safety number section (12 groups of 5 digits) with a link to the verification screen.
class _SafetySection extends ConsumerStatefulWidget {
  const _SafetySection({required this.accountId, required this.name});
  final String accountId;
  final String name;

  @override
  ConsumerState<_SafetySection> createState() => _SafetySectionState();
}

class _SafetySectionState extends ConsumerState<_SafetySection> {
  SafetyNumber? _sn;

  @override
  void initState() {
    super.initState();
    backend.getSafetyNumber(widget.accountId).then((s) {
      if (mounted) setState(() => _sn = s);
    }).catchError((Object _) {});
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    final verified = ref.watch(storeProvider.select((s) => s.contacts[widget.accountId]?.verified ?? false));
    final groups = _sn?.groups ?? const <String>[];
    return Sect(
      padding: const EdgeInsets.all(16),
      child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
        Row(children: [
          AppIcon('shield', size: 17, color: t.accentText),
          const SizedBox(width: 8),
          Expanded(child: Text(context.t('safety.number'), style: ts(14.5, weight: 650, color: t.ink))),
          if (verified) SoftBadge(context.t('safety.verified')),
        ]),
        if (groups.isNotEmpty) Padding(padding: const EdgeInsets.symmetric(vertical: 14), child: SafetyGrid(groups: groups)),
        Text(context.t('safety.compare', args: {'name': widget.name}), style: ts(12.5, color: t.muted, height: 1.45)),
        const SizedBox(height: 12),
        ChipBtn(
          label: context.t(verified ? 'safety.review' : 'safety.verify'),
          icon: 'scan',
          onPressed: () => Navigator.of(context).push(MaterialPageRoute<void>(builder: (_) => SafetyNumberPage(accountId: widget.accountId, name: widget.name))),
        ),
      ]),
    );
  }
}

class SafetyGrid extends StatelessWidget {
  const SafetyGrid({super.key, required this.groups, this.big = false});
  final List<String> groups;
  final bool big;

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    return LayoutBuilder(builder: (context, c) {
      final w = (c.maxWidth - 30) / 4;
      return Wrap(spacing: 10, runSpacing: 6, children: [
        for (final g in groups)
          SizedBox(width: w, child: Text(g, textAlign: big ? TextAlign.center : TextAlign.start, style: ts(big ? 16 : 14, color: t.ink, mono: true, letterSpacing: .56))),
      ]);
    });
  }
}

class SafetyNumberPage extends ConsumerStatefulWidget {
  const SafetyNumberPage({super.key, required this.accountId, required this.name});
  final String accountId;
  final String name;

  @override
  ConsumerState<SafetyNumberPage> createState() => _SafetyNumberPageState();
}

class _SafetyNumberPageState extends ConsumerState<SafetyNumberPage> {
  SafetyNumber? _sn;
  String? _scan;
  bool _busy = false;

  @override
  void initState() {
    super.initState();
    attempt(() => backend.getSafetyNumber(widget.accountId)).then((s) {
      if (mounted && s != null) setState(() => _sn = s);
    });
  }

  Future<void> _setVerified(bool v) async {
    setState(() => _busy = true);
    final c = await attempt(() => backend.setVerified(widget.accountId, v));
    if (!mounted) return;
    setState(() => _busy = false);
    if (c != null) {
      store.upsertContact(c);
      setState(() => _sn = SafetyNumber(digits: _sn!.digits, qrPayload: _sn!.qrPayload, verified: v));
    }
  }

  void _openScanner() {
    showAppDialog<void>(context, title: context.t('safety.scan'), builder: (ctx) => QrScanner(
          submitLabel: ctx.t('common.continue'),
          onResult: (text) {
            Navigator.of(ctx).pop();
            final ok = text.trim() == _sn?.qrPayload;
            setState(() => _scan = ok ? 'match' : 'mismatch');
            if (ok && !(_sn?.verified ?? false)) _setVerified(true);
          },
        ));
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    final sn = _sn;
    return PaneScaffold(
      header: PaneHeader(title: context.t('safety.title')),
      body: sn == null
          ? const Spinner()
          : ListView(padding: const EdgeInsets.fromLTRB(20, 24, 20, 24), children: [
              if (sn.verified)
                Center(
                  child: Container(
                    padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 4),
                    decoration: BoxDecoration(color: t.accentSoft, borderRadius: BorderRadius.circular(6)),
                    child: Row(mainAxisSize: MainAxisSize.min, children: [
                      AppIcon('shield', size: 14, color: t.accentText),
                      const SizedBox(width: 6),
                      Text(context.t('safety.verified').toUpperCase(), style: ts(11.5, weight: 700, color: t.accentText, letterSpacing: .5)),
                    ]),
                  ),
                ),
              const SizedBox(height: 18),
              Center(child: QrCodeView(data: sn.qrPayload, size: 180)),
              const SizedBox(height: 18),
              SafetyGrid(groups: sn.groups, big: true),
              const SizedBox(height: 18),
              Text(context.t('safety.explanation', args: {'name': widget.name}), textAlign: TextAlign.center, style: ts(13.5, color: t.muted, height: 1.45)),
              KtContactNotice(accountId: widget.accountId),
              if (_scan != null) ...[
                const SizedBox(height: 18),
                NoticeBox(
                  tone: _scan == 'match' ? 'info' : 'danger',
                  icon: _scan == 'match' ? 'okcircle' : 'alert',
                  text: _scan == 'match' ? context.t('safety.match') : context.t('safety.mismatch', args: {'name': widget.name}),
                ),
              ],
              const SizedBox(height: 18),
              Btn(label: context.t('safety.scan'), icon: 'scan', kind: BtnKind.secondary, height: 48, onPressed: _openScanner),
              const SizedBox(height: 10),
              Btn(
                label: context.t(sn.verified ? 'safety.clear' : 'safety.markVerified'),
                icon: 'shield',
                kind: sn.verified ? BtnKind.secondary : BtnKind.primary,
                height: 48,
                busy: _busy,
                onPressed: () => _setVerified(!sn.verified),
              ),
            ]),
    );
  }
}

// ---------------------------------------------------------------------------
// group
// ---------------------------------------------------------------------------

Future<void> _updateGroup(String conversationId, {String? name, String? description, String? avatarPath, GroupSettings? settings}) async {
  final g = await attempt(() => backend.updateGroup(conversationId, name: name, description: description, avatarPath: avatarPath, settings: settings));
  if (g != null) store.upsertGroup(g);
}

class _GroupInfo extends ConsumerStatefulWidget {
  const _GroupInfo({required this.conv});
  final Conversation conv;

  @override
  ConsumerState<_GroupInfo> createState() => _GroupInfoState();
}

class _GroupInfoState extends ConsumerState<_GroupInfo> {
  @override
  void initState() {
    super.initState();
    loadGroup(widget.conv.id);
  }

  Future<void> _addMembers(GroupInfo group) async {
    final selected = await Navigator.of(context).push<List<String>>(MaterialPageRoute(builder: (_) => _AddMembersPage(group: group)));
    if (selected == null || selected.isEmpty) return;
    final g = await attempt(() => backend.addGroupMembers(group.conversationId, selected));
    if (g != null) store.upsertGroup(g);
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    final conv = widget.conv;
    final group = ref.watch(storeProvider.select((s) => s.groups[conv.id]));
    final contacts = ref.watch(storeProvider.select((s) => s.contacts));
    final profile = ref.watch(storeProvider.select((s) => s.profile));
    if (group == null) return const Spinner();
    final lang = context.lang;
    final amAdmin = group.myRole == 'admin' && !conv.left;
    final canEdit = !conv.left && (amAdmin || !group.settings.onlyAdminsEdit);
    final members = [...group.members]..sort((a, b) {
        if (a.isMe != b.isMe) return a.isMe ? -1 : 1;
        if (a.isAdmin != b.isAdmin) return a.isAdmin ? -1 : 1;
        return a.displayName.toLowerCase().compareTo(b.displayName.toLowerCase());
      });
    final me = members.where((m) => m.isMe).firstOrNull;
    final creator = group.createdBy == null
        ? null
        : (group.members.where((m) => m.accountId == group.createdBy).firstOrNull?.displayName ?? contacts[group.createdBy]?.displayName);
    final muted = conv.isMuted();

    Widget avatar = Avatar(name: group.name, path: group.avatarPath, seed: conv.id, size: 112, group: true);
    if (canEdit) {
      avatar = GestureDetector(
        onTap: () => showActionSheet(context, actions: [
          SheetAction(
            label: context.t('group.changePhoto'),
            icon: 'image',
            onTap: () async {
              final p = await pickImagePath();
              if (p != null) await _updateGroup(conv.id, avatarPath: p);
            },
          ),
          SheetAction(
            label: context.t('profile.removePhoto'),
            icon: 'trash',
            danger: true,
            hidden: group.avatarPath == null,
            onTap: () => _updateGroup(conv.id, avatarPath: ''),
          ),
        ]),
        child: Stack(clipBehavior: Clip.none, children: [
          avatar,
          Positioned(
            right: -4,
            bottom: -2,
            child: Container(
              width: 42,
              height: 42,
              decoration: BoxDecoration(color: t.accent, shape: BoxShape.circle, border: Border.all(color: t.pane, width: 4)),
              child: Center(child: AppIcon('camera', size: 15, color: t.onAccent)),
            ),
          ),
        ]),
      );
    }

    return Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
      _Hero(
        avatar: avatar,
        name: group.name,
        nameTrailing: [
          if (canEdit) ...[
            const SizedBox(width: 4),
            IconBtn('edit', size: 32, iconSize: 16, color: t.muted, tooltip: context.t('group.rename'), onPressed: () async {
              final v = await showTextEdit(context, title: context.t('group.name'), label: context.t('group.name'), initial: group.name, maxLength: 80);
              if (v != null && v.isNotEmpty) await _updateGroup(conv.id, name: v);
            }),
          ],
        ],
        sub: conv.left ? context.t('group.notMember') : context.t('group.subtitle', count: group.memberCount),
        actions: [
          _Action('search', context.t('info.search'), () => _searchInChat(context, conv.id)),
          if (amAdmin) _Action('plus', context.t('info.add'), () => _addMembers(group)),
          _Action(muted ? 'bell' : 'mute', context.t(muted ? 'info.unmute' : 'info.mute'), () => toggleMute(context, conv)),
        ],
      ),
      Sect(
        child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
          Row(children: [
            Expanded(child: Text(context.t('group.description'), style: ts(12.5, weight: 600, color: t.muted))),
            if (canEdit)
              IconBtn('edit', size: 28, iconSize: 15, color: t.muted, tooltip: context.t('group.editDescription'), onPressed: () async {
                final v = await showTextEdit(
                  context,
                  title: context.t('group.description'),
                  label: context.t('group.description'),
                  initial: group.description,
                  multiline: true,
                  maxLength: 500,
                  allowEmpty: true,
                );
                if (v != null) await _updateGroup(conv.id, description: v);
              }),
          ]),
          const SizedBox(height: 4),
          Text(
            group.description.isNotEmpty ? group.description : context.t(canEdit ? 'group.addDescription' : 'group.noDescription'),
            style: ts(14.5, color: group.description.isNotEmpty ? t.ink : t.muted, height: 1.45),
          ),
          const SizedBox(height: 8),
          Text(
            creator != null
                ? context.t('group.createdBy', args: {
                    'name': group.createdBy == me?.accountId ? context.t('common.you') : creator,
                    'date': formatDateTime(group.createdAt, lang),
                  })
                : context.t('group.createdAt', args: {'date': formatDateTime(group.createdAt, lang)}),
            style: ts(12, color: t.faint),
          ),
        ]),
      ),
      _MediaSection(conversationId: conv.id),
      _ChatToggles(conv: conv),
      if (amAdmin)
        Sect(
          padding: const EdgeInsets.only(bottom: 4),
          child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
            Padding(
              padding: const EdgeInsets.fromLTRB(16, 12, 16, 0),
              child: Text(context.t('group.settings'), style: ts(12.5, weight: 600, color: t.muted)),
            ),
            ToggleRow(
              label: context.t('group.onlyAdminsSend'),
              sub: context.t('group.onlyAdminsSendHint'),
              on: group.settings.onlyAdminsSend,
              onToggle: () => _updateGroup(conv.id, settings: group.settings.copyWith(onlyAdminsSend: !group.settings.onlyAdminsSend)),
            ),
            ToggleRow(
              label: context.t('group.onlyAdminsEdit'),
              sub: context.t('group.onlyAdminsEditHint'),
              on: group.settings.onlyAdminsEdit,
              onToggle: () => _updateGroup(conv.id, settings: group.settings.copyWith(onlyAdminsEdit: !group.settings.onlyAdminsEdit)),
            ),
            ToggleRow(
              label: context.t('groupRules.showMembers'),
              sub: context.t('groupRules.showMembersHint'),
              on: !group.settings.hideMembers,
              onToggle: () => _updateGroup(conv.id, settings: group.settings.copyWith(hideMembers: !group.settings.hideMembers)),
            ),
            ListRow(
              icon: 'link',
              title: context.t('invite.title'),
              sub: context.t(group.inviteLink == null ? 'invite.off' : group.inviteApproval ? 'invite.onApproval' : 'invite.on'),
              onTap: () => openInviteLink(context, conv.id),
            ),
            if (group.inviteApproval || group.joinRequests > 0)
              ListRow(
                icon: 'users',
                title: context.t('invite.requests'),
                onTap: () => openJoinRequests(context, conv.id),
                trailing: group.joinRequests > 0 ? SoftBadge('${group.joinRequests}') : null,
              ),
            ListRow(
              icon: 'x',
              title: context.t('invite.bannedTitle'),
              onTap: () => openGroupBans(context, conv.id),
            ),
            ToggleRow(
              label: context.t('groupRules.firstNames'),
              sub: context.t('groupRules.firstNamesHint'),
              on: group.settings.firstNames,
              onToggle: () => _updateGroup(conv.id,
                  settings: group.settings.copyWith(memberNames: group.settings.firstNames ? 'number' : 'first_name')),
            ),
          ]),
        ),
      Sect(
        padding: const EdgeInsets.fromLTRB(0, 14, 0, 6),
        child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
          Padding(
            padding: const EdgeInsets.fromLTRB(16, 0, 16, 8),
            child: Text(context.t('group.members', count: group.memberCount), style: ts(14.5, weight: 650, color: t.ink)),
          ),
          if (group.membersHidden)
            Padding(
              padding: const EdgeInsets.fromLTRB(16, 0, 16, 8),
              child: Text(context.t('groupRules.hiddenNotice'), style: ts(12.5, color: t.muted)),
            ),
          if (amAdmin)
            Tap(
              onTap: () => _addMembers(group),
              padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 8),
              child: Row(children: [
                Container(
                  width: 40,
                  height: 40,
                  decoration: BoxDecoration(color: t.accentSoft, shape: BoxShape.circle),
                  child: Center(child: AppIcon('plus', size: 19, color: t.accentText)),
                ),
                const SizedBox(width: 12),
                Text(context.t('group.addMembers'), style: ts(14.5, weight: 600, color: t.accentText)),
              ]),
            ),
          for (final m in members) _MemberRow(m: m, group: group, amAdmin: amAdmin, contact: contacts[m.accountId], profile: profile),
        ]),
      ),
      Sect(
        padding: const EdgeInsets.symmetric(vertical: 4),
        child: !conv.left
            ? _DangerRow(icon: 'logout', label: context.t('group.leave.action'), onTap: () => confirmLeaveGroup(context, conv))
            : _DangerRow(
                icon: 'trash',
                label: context.t('chat.delete.action'),
                onTap: () async {
                  final nav = Navigator.of(context);
                  if (await confirmDeleteChat(context, conv)) nav.popUntil((r) => r.isFirst);
                },
              ),
      ),
    ]);
  }
}

class _MemberRow extends StatelessWidget {
  const _MemberRow({required this.m, required this.group, required this.amAdmin, this.contact, this.profile});
  final GroupMember m;
  final GroupInfo group;
  final bool amAdmin;
  final Contact? contact;
  final Profile? profile;

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    // First-name groups: only the name the group shows, never the number or a local nickname.
    final firstNames = group.settings.firstNames;
    final name = m.isMe
        ? context.t('common.you')
        : firstNames
            ? (m.displayName == 'Member' ? context.t('groupRules.unnamed') : m.displayName)
            : (contact?.displayName ?? m.displayName);
    final avatarName = m.isMe ? (profile?.name ?? m.displayName) : name;
    final avatarPath = m.isMe ? profile?.avatarPath : (contact?.avatarPath ?? m.avatarPath);
    final about = m.isMe ? profile?.about : contact?.about;
    final status = firstNames && !m.isMe ? '' : ((about?.isNotEmpty ?? false) ? about! : formatAccountNumber(m.number));
    final cid = group.conversationId;
    Future<void> admin() async {
      final g = await attempt(() => backend.setGroupAdmin(cid, m.accountId, !m.isAdmin));
      if (g != null) store.upsertGroup(g);
    }

    Future<void> remove() async {
      final ok = await showConfirm(
        context,
        title: context.t('group.removeTitle', args: {'name': name}),
        body: context.t('group.removeBody', args: {'name': name, 'group': group.name}),
        confirmLabel: context.t('common.remove'),
      );
      if (!ok) return;
      final g = await attempt(() => backend.removeGroupMember(cid, m.accountId));
      if (g != null) store.upsertGroup(g);
    }

    return Tap(
      onTap: m.isMe
          ? null
          : () => showActionSheet(context, title: name, actions: [
                SheetAction(
                  label: context.t('group.messageMember', args: {'name': name}),
                  icon: 'chat',
                  hidden: firstNames,
                  onTap: () => openDirect(m.accountId),
                ),
                SheetAction(label: context.t(m.isAdmin ? 'group.dismissAdmin' : 'group.makeAdmin'), icon: m.isAdmin ? 'shieldoff' : 'crown', hidden: !amAdmin, onTap: admin),
                SheetAction(label: context.t('group.remove'), icon: 'userminus', danger: true, hidden: !amAdmin, onTap: remove),
              ]),
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 8),
      child: Row(children: [
        Avatar(name: avatarName, path: avatarPath, seed: m.isMe ? 'me' : m.accountId, size: 40),
        const SizedBox(width: 12),
        Expanded(
          child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
            Text(name, maxLines: 1, overflow: TextOverflow.ellipsis, style: ts(14.5, weight: 600, color: t.ink)),
            Text(status, maxLines: 1, overflow: TextOverflow.ellipsis, style: ts(12.5, color: t.muted)),
          ]),
        ),
        if (m.isAdmin) SoftBadge(context.t('group.admin')),
        if (!m.verified) SoftBadge(context.t('group.unverified')),
      ]),
    );
  }
}

class _AddMembersPage extends StatefulWidget {
  const _AddMembersPage({required this.group});
  final GroupInfo group;

  @override
  State<_AddMembersPage> createState() => _AddMembersPageState();
}

class _AddMembersPageState extends State<_AddMembersPage> {
  List<String> _selected = [];

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    return PaneScaffold(
      header: PaneHeader(title: context.t('group.addMembers'), backIcon: 'x'),
      body: ContactPicker(
        selected: _selected,
        exclude: widget.group.members.map((m) => m.accountId).toList(),
        onChanged: (v) => setState(() => _selected = v),
      ),
      bottom: Container(
        padding: EdgeInsets.fromLTRB(16, 12, 16, 12 + MediaQuery.of(context).padding.bottom),
        decoration: BoxDecoration(color: t.pane, border: Border(top: BorderSide(color: t.line))),
        child: Btn(
          label: '${context.t('common.add')}${_selected.isEmpty ? '' : ' (${_selected.length})'}',
          icon: 'plus',
          height: 48,
          onPressed: _selected.isEmpty ? null : () => Navigator.of(context).pop(_selected),
        ),
      ),
    );
  }
}

// ---------------------------------------------------------------------------
// media gallery
// ---------------------------------------------------------------------------

class MediaGalleryPage extends StatefulWidget {
  const MediaGalleryPage({super.key, required this.conversationId});
  final String conversationId;

  @override
  State<MediaGalleryPage> createState() => _MediaGalleryPageState();
}

class _MediaGalleryPageState extends State<MediaGalleryPage> {
  String _tab = 'media';
  final _lists = <String, List<Message>?>{'media': null, 'files': null};

  @override
  void initState() {
    super.initState();
    for (final kind in const ['media', 'files']) {
      backend.listMedia(widget.conversationId, kind, limit: 200).then((m) {
        if (mounted) setState(() => _lists[kind] = m);
      }).catchError((Object _) {
        if (mounted) setState(() => _lists[kind] = const []);
      });
    }
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    final list = _lists[_tab];
    Widget tab(String key, String icon, String label) => Expanded(
          child: GestureDetector(
            behavior: HitTestBehavior.opaque,
            onTap: () => setState(() => _tab = key),
            child: Container(
              height: 44,
              decoration: BoxDecoration(border: Border(bottom: BorderSide(color: _tab == key ? t.accent : t.line, width: _tab == key ? 2 : 1))),
              child: Row(mainAxisAlignment: MainAxisAlignment.center, children: [
                AppIcon(icon, size: 16, color: _tab == key ? t.accentText : t.muted),
                const SizedBox(width: 8),
                Text(label, style: ts(14, weight: 650, color: _tab == key ? t.accentText : t.muted)),
              ]),
            ),
          ),
        );
    Widget body;
    if (list == null) {
      body = const Spinner();
    } else if (_tab == 'media') {
      final items = <(Message, Attachment)>[
        for (final m in list)
          for (final a in m.attachments.where((a) => a.isImage || a.isVideo)) (m, a),
      ];
      body = items.isEmpty
          ? Padding(padding: const EdgeInsets.all(40), child: Text(context.t('info.noMedia'), textAlign: TextAlign.center, style: ts(14, color: t.muted)))
          : GridView.count(
              crossAxisCount: 3,
              padding: const EdgeInsets.all(12),
              mainAxisSpacing: 4,
              crossAxisSpacing: 4,
              children: [for (final (m, a) in items) _Thumb(m: m, a: a, index: 0)],
            );
    } else {
      final lang = context.lang;
      body = list.isEmpty
          ? Padding(padding: const EdgeInsets.all(40), child: Text(context.t('info.noFiles'), textAlign: TextAlign.center, style: ts(14, color: t.muted)))
          : ListView(padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 8), children: [
              for (final m in list)
                for (final a in m.attachments)
                  Container(
                    margin: const EdgeInsets.only(bottom: 4),
                    padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 6),
                    decoration: BoxDecoration(color: t.sunk, borderRadius: BorderRadius.circular(12)),
                    child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
                      FileCard(message: m, a: a, metaColor: t.muted, iconBg: t.pane),
                      Text(formatDateTime(m.sentAt, lang), style: ts(11.5, color: t.muted)),
                    ]),
                  ),
            ]);
    }
    return PaneScaffold(
      header: PaneHeader(title: context.t('info.media')),
      body: Column(children: [
        Row(children: [tab('media', 'image', context.t('info.tabMedia')), tab('files', 'file', context.t('info.tabFiles'))]),
        Expanded(child: body),
      ]),
    );
  }
}
