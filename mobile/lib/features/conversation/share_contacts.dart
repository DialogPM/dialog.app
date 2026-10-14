// Sharing contacts: the picker page and the contact cards shown in messages.

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../app/globals.dart';
import '../../app/nav.dart';
import '../../core/models.dart';
import '../../l10n/i18n.dart';
import '../../state/actions.dart';
import '../../state/store.dart';
import '../../theme/app_theme.dart';
import '../../util/format.dart';
import '../../widgets/toast.dart';
import '../../widgets/ui.dart';
import '../chats/contact_picker.dart';

const _maxContacts = 50;

/// Picks one or more people (contacts and chat partners) and sends them as contact cards.
class ShareContactsPage extends StatefulWidget {
  const ShareContactsPage({super.key, required this.conversationId});
  final String conversationId;

  @override
  State<ShareContactsPage> createState() => _ShareContactsPageState();
}

class _ShareContactsPageState extends State<ShareContactsPage> {
  List<String> _selected = [];
  bool _busy = false;

  Future<void> _send() async {
    setState(() => _busy = true);
    final m = await sendMessage(conversationId: widget.conversationId, contactIds: _selected);
    if (!mounted) return;
    setState(() => _busy = false);
    if (m != null) Navigator.of(context).pop();
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    return PaneScaffold(
      header: PaneHeader(title: context.t('shareContacts.title')),
      body: ContactPicker(
        includeChats: true,
        selected: _selected,
        onChanged: (v) => setState(() => _selected = v.take(_maxContacts).toList()),
      ),
      bottom: Container(
        padding: EdgeInsets.fromLTRB(16, 12, 16, 12 + MediaQuery.of(context).padding.bottom),
        decoration: BoxDecoration(color: t.pane, border: Border(top: BorderSide(color: t.line))),
        child: Btn(
          label: _selected.isEmpty
              ? context.t('shareContacts.send')
              : context.t('shareContacts.sendCount', count: _selected.length),
          height: 48,
          busy: _busy,
          onPressed: _selected.isEmpty || _busy ? null : _send,
        ),
      ),
    );
  }
}

/// Contact cards inside a message bubble.
class ContactCards extends StatelessWidget {
  const ContactCards({super.key, required this.contacts, required this.fg, required this.meta, required this.link});
  final List<SharedContact> contacts;
  final Color fg;
  final Color meta;
  final Color link;

  @override
  Widget build(BuildContext context) => SizedBox(
        width: 270,
        child: Column(children: [
          for (final c in contacts) _ContactCard(c: c, fg: fg, meta: meta, link: link),
        ]),
      );
}

class _ContactCard extends ConsumerStatefulWidget {
  const _ContactCard({required this.c, required this.fg, required this.meta, required this.link});
  final SharedContact c;
  final Color fg;
  final Color meta;
  final Color link;

  @override
  ConsumerState<_ContactCard> createState() => _ContactCardState();
}

class _ContactCardState extends ConsumerState<_ContactCard> {
  bool _busy = false;

  /// Adds the person (saved under the card's name). The number is resolved by the server and
  /// that account, not the id written in the card, is the one we open.
  Future<String?> _ensureContact(Contact? known) async {
    if (known != null) return known.accountId;
    final added = await attempt(() => backend.addContactByNumber(widget.c.number, name: widget.c.name));
    if (added == null) return null;
    store.upsertContact(added);
    return added.accountId;
  }

  Future<void> _message(Contact? known) async {
    setState(() => _busy = true);
    final id = await _ensureContact(known);
    final conv = id == null ? null : await openDirectConversation(id);
    if (mounted) setState(() => _busy = false);
    if (conv != null) openChat(conv.id);
  }

  Future<void> _add() async {
    setState(() => _busy = true);
    final id = await _ensureContact(null);
    if (mounted) setState(() => _busy = false);
    if (id != null && mounted) showToast(context.t('contactCard.added'));
  }

  @override
  Widget build(BuildContext context) {
    final c = widget.c;
    final known = ref.watch(storeProvider.select((s) => s.contacts[c.accountId]));
    final isMe = ref.watch(storeProvider.select((s) => s.appState?.account?.accountId == c.accountId));
    final name = known?.localName ?? known?.profileName ?? c.name;
    final line = widget.fg.withValues(alpha: 0.12);
    Widget action(String label, VoidCallback onTap) => Expanded(
          child: InkWell(
            onTap: _busy ? null : onTap,
            child: Padding(
              padding: const EdgeInsets.symmetric(vertical: 10),
              child: Center(child: Text(label, style: ts(13.5, weight: 700, color: _busy ? widget.meta : widget.link))),
            ),
          ),
        );
    return Container(
      margin: const EdgeInsets.only(top: 4, bottom: 2),
      decoration: BoxDecoration(color: widget.fg.withValues(alpha: 0.06), borderRadius: BorderRadius.circular(12)),
      clipBehavior: Clip.antiAlias,
      child: Column(children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(12, 10, 12, 10),
          child: Row(children: [
            Avatar(name: name, path: known?.avatarPath, seed: c.accountId, size: 42),
            const SizedBox(width: 11),
            Expanded(
              child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
                Text(isMe ? '$name (${context.t('contactCard.you')})' : name,
                    maxLines: 1, overflow: TextOverflow.ellipsis, style: ts(14.5, weight: 700, color: widget.fg)),
                const SizedBox(height: 2),
                Text(formatAccountNumber(c.number), style: ts(12, color: widget.meta, mono: true)),
              ]),
            ),
          ]),
        ),
        if (!isMe)
          DecoratedBox(
            decoration: BoxDecoration(border: Border(top: BorderSide(color: line))),
            child: Row(children: [
              action(context.t('contactCard.message'), () => _message(known)),
              if (known == null) ...[
                Container(width: 1, height: 38, color: line),
                action(context.t('contactCard.add'), _add),
              ],
            ]),
          ),
      ]),
    );
  }
}
