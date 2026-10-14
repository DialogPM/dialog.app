// Contact rows and the multi-select contact picker (new group, add members).

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../core/models.dart';
import '../../l10n/i18n.dart';
import '../../state/store.dart';
import '../../theme/app_theme.dart';
import '../../util/format.dart';
import '../../widgets/app_icon.dart';
import '../../widgets/ui.dart';
import 'chat_list.dart';

/// Contacts plus people from direct chats who were never saved as contacts (they wrote first).
Map<String, Contact> withChatPeers(Map<String, Contact> contacts, Iterable<Conversation> conversations) {
  final all = {...contacts};
  for (final c in conversations) {
    final id = c.peerAccountId, number = c.peerNumber;
    if (c.kind != 'direct' || id == null || number == null || all.containsKey(id)) continue;
    all[id] = Contact(accountId: id, number: number, displayName: c.title, avatarPath: c.avatarPath, blocked: false, verified: false);
  }
  return all;
}

/// Contacts filtered by name / number / phone, sorted by name.
List<Contact> sortedContacts(Map<String, Contact> contacts, String filter) {
  final q = filter.trim().toLowerCase();
  final digits = q.replaceAll(RegExp(r'\s'), '');
  final isDigits = digits.isNotEmpty && RegExp(r'^\d+$').hasMatch(digits);
  return contacts.values
      .where((c) =>
          q.isEmpty ||
          c.displayName.toLowerCase().contains(q) ||
          (isDigits && c.number.contains(digits)) ||
          (c.phone != null && digits.isNotEmpty && c.phone!.replaceAll(RegExp(r'\s'), '').contains(digits)))
      .toList()
    ..sort((a, b) => a.displayName.toLowerCase().compareTo(b.displayName.toLowerCase()));
}

/// Generic row (.row): 60px min height, avatar/icon, title and subtitle.
class PickRow extends StatelessWidget {
  const PickRow({super.key, required this.leading, required this.title, this.sub, this.onTap, this.trailing, this.titleColor});

  final Widget leading;
  final String title;
  final String? sub;
  final VoidCallback? onTap;
  final Widget? trailing;
  final Color? titleColor;

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    return Padding(
      padding: EdgeInsets.symmetric(horizontal: t.rowMx),
      child: Tap(
        onTap: onTap,
        radius: BorderRadius.circular(t.rowRadius),
        child: Container(
          constraints: const BoxConstraints(minHeight: 60),
          padding: EdgeInsets.symmetric(horizontal: t.rowPx, vertical: 10),
          child: Row(children: [
            leading,
            const SizedBox(width: 13),
            Expanded(
              child: Column(crossAxisAlignment: CrossAxisAlignment.start, mainAxisSize: MainAxisSize.min, children: [
                Text(title, maxLines: 1, overflow: TextOverflow.ellipsis, style: ts(15, weight: 600, color: titleColor ?? t.ink)),
                if (sub != null) Text(sub!, maxLines: 2, overflow: TextOverflow.ellipsis, style: ts(12.5, color: t.muted, height: 1.4)),
              ]),
            ),
            if (trailing != null) ...[const SizedBox(width: 10), trailing!],
          ]),
        ),
      ),
    );
  }
}

/// Round 44px icon of the action rows (accent, or soft).
class RowIcon extends StatelessWidget {
  const RowIcon(this.icon, {super.key, this.soft = false});
  final String icon;
  final bool soft;

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    return Container(
      width: 44,
      height: 44,
      decoration: BoxDecoration(shape: BoxShape.circle, color: soft ? t.accentSoft : t.accent),
      child: Center(child: AppIcon(icon, size: 21, color: soft ? t.accentText : t.onAccent)),
    );
  }
}

class ContactRow extends StatelessWidget {
  const ContactRow({super.key, required this.c, this.onTap, this.trailing, this.avatarSize = 44});
  final Contact c;
  final VoidCallback? onTap;
  final Widget? trailing;
  final double avatarSize;

  @override
  Widget build(BuildContext context) => PickRow(
        leading: Avatar(name: c.displayName, path: c.avatarPath, seed: c.accountId, size: avatarSize),
        title: c.displayName,
        sub: (c.about?.isNotEmpty ?? false) ? c.about : formatAccountNumber(c.number),
        onTap: onTap,
        trailing: trailing ?? (c.blocked ? SoftBadge(context.t('chat.block.blocked'), tone: 'danger') : null),
      );
}

/// Multi-select list of contacts with search and removable chips.
class ContactPicker extends ConsumerStatefulWidget {
  const ContactPicker({super.key, required this.selected, required this.onChanged, this.exclude = const [], this.includeChats = false});

  final List<String> selected;
  final ValueChanged<List<String>> onChanged;

  /// Account ids that cannot be picked (already members).
  final List<String> exclude;

  /// Also offer people from direct chats who are not saved as contacts.
  final bool includeChats;

  @override
  ConsumerState<ContactPicker> createState() => _ContactPickerState();
}

class _ContactPickerState extends ConsumerState<ContactPicker> {
  final _query = TextEditingController();

  @override
  void dispose() {
    _query.dispose();
    super.dispose();
  }

  void _toggle(String id) => widget.onChanged(
        widget.selected.contains(id) ? widget.selected.where((x) => x != id).toList() : [...widget.selected, id],
      );

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    final saved = ref.watch(storeProvider.select((s) => s.contacts));
    final convs = ref.watch(storeProvider.select((s) => s.conversations));
    final all = widget.includeChats ? withChatPeers(saved, convs.values) : saved;
    final contacts = sortedContacts(all, _query.text).where((c) => !c.blocked && !widget.exclude.contains(c.accountId)).toList();
    return Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
      if (widget.selected.isNotEmpty)
        Padding(
          padding: const EdgeInsets.fromLTRB(16, 10, 16, 0),
          child: Wrap(spacing: 6, runSpacing: 6, children: [
            for (final id in widget.selected)
              Tap(
                onTap: () => _toggle(id),
                color: t.accentSoft,
                radius: BorderRadius.circular(999),
                semanticLabel: context.t('common.remove'),
                child: Container(
                  height: 30,
                  padding: const EdgeInsets.only(left: 4, right: 6),
                  child: Row(mainAxisSize: MainAxisSize.min, children: [
                    Avatar(name: all[id]?.displayName ?? id, seed: id, size: 22, path: all[id]?.avatarPath),
                    const SizedBox(width: 6),
                    Text(all[id]?.displayName ?? id, style: ts(13, weight: 600, color: t.accentText)),
                    const SizedBox(width: 6),
                    AppIcon('x', size: 14, color: t.accentText),
                  ]),
                ),
              ),
          ]),
        ),
      Padding(
        padding: const EdgeInsets.fromLTRB(16, 12, 16, 8),
        child: SearchBox(controller: _query, hint: context.t('newChat.search'), onChanged: (_) => setState(() {})),
      ),
      Expanded(
        child: contacts.isEmpty
            ? Padding(
                padding: const EdgeInsets.all(40),
                child: Text(context.t(_query.text.isEmpty ? 'newChat.noContacts' : 'chats.noResults'),
                    textAlign: TextAlign.center, style: ts(14, color: t.muted)),
              )
            : ListView.builder(
                padding: const EdgeInsets.only(bottom: 8),
                itemCount: contacts.length,
                itemBuilder: (_, i) {
                  final c = contacts[i];
                  return ContactRow(
                    c: c,
                    avatarSize: 40,
                    onTap: () => _toggle(c.accountId),
                    trailing: CheckBox(on: widget.selected.contains(c.accountId), round: true),
                  );
                },
              ),
      ),
    ]);
  }
}
