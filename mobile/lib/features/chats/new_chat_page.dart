// "New chat": new group, add by account number, scan a QR code / paste a link, address book,
// and the contact list.

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../app/globals.dart';
import '../../app/nav.dart';
import '../../l10n/i18n.dart';
import '../../state/actions.dart';
import '../../state/store.dart';
import '../../theme/app_theme.dart';
import '../../util/format.dart';
import '../../widgets/ui.dart';
import 'chat_list.dart';
import 'contact_picker.dart';

class NewChatPage extends ConsumerStatefulWidget {
  const NewChatPage({super.key});

  @override
  ConsumerState<NewChatPage> createState() => _NewChatPageState();
}

class _NewChatPageState extends ConsumerState<NewChatPage> {
  final _query = TextEditingController();
  final _number = TextEditingController();
  final _name = TextEditingController();
  bool _addOpen = false;
  bool _busy = false;
  String? _error;

  @override
  void dispose() {
    _query.dispose();
    _number.dispose();
    _name.dispose();
    super.dispose();
  }

  Future<void> _add() async {
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      final name = _name.text.trim();
      final c = await backend.addContactByNumber(digitsOnly(_number.text), name: name.isEmpty ? null : name);
      store.upsertContact(c);
      _number.clear();
      _name.clear();
      setState(() => _addOpen = false);
      await openDirect(c.accountId);
    } catch (e) {
      setState(() => _error = errorText(e));
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    final contacts = sortedContacts(ref.watch(storeProvider.select((s) => s.contacts)), _query.text);
    final q = _query.text;
    return PaneScaffold(
      header: PaneHeader(title: context.t('newChat.title')),
      body: ListView(padding: const EdgeInsets.only(bottom: 12), children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(16, 12, 16, 8),
          child: SearchBox(controller: _query, hint: context.t('newChat.search'), onChanged: (_) => setState(() {})),
        ),
        if (q.isEmpty) ...[
          PickRow(leading: const RowIcon('users'), title: context.t('newGroup.title'), onTap: openNewGroup),
          PickRow(
            leading: const RowIcon('plus'),
            title: context.t('newChat.byNumber'),
            sub: context.t('newChat.byNumberHint'),
            onTap: () => setState(() => _addOpen = !_addOpen),
          ),
          AnimatedSize(
            duration: const Duration(milliseconds: 180),
            alignment: Alignment.topCenter,
            child: !_addOpen
                ? const SizedBox(width: double.infinity)
                : Padding(
                    padding: const EdgeInsets.fromLTRB(20, 8, 20, 16),
                    child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
                      AccountNumberField(
                        controller: _number,
                        label: context.t('onboarding.accountNumber'),
                        large: false,
                        autofocus: true,
                        onChanged: (_) => setState(() {}),
                      ),
                      const SizedBox(height: 12),
                      AppField(controller: _name, label: context.t('newChat.nameOptional'), maxLength: 60),
                      if (_error != null) ...[const SizedBox(height: 10), Text(_error!, style: ts(13, color: t.danger))],
                      const SizedBox(height: 12),
                      Btn(
                        label: context.t('newChat.startChat'),
                        height: 44,
                        busy: _busy,
                        onPressed: digitsOnly(_number.text).length == 16 ? _add : null,
                      ),
                    ]),
                  ),
          ),
          PickRow(
            leading: const RowIcon('scan'),
            title: context.t('newChat.byQr'),
            sub: context.t('newChat.byQrHint'),
            onTap: () => openQr(tab: 'scan'),
          ),
          PickRow(
            leading: const RowIcon('book', soft: true),
            title: context.t('newChat.byPhone'),
            sub: context.t('newChat.byPhoneHint'),
            onTap: () => openSettingsSection('chats'),
          ),
        ],
        SectionLabel(context.t('newChat.contacts')),
        if (contacts.isEmpty)
          Padding(
            padding: const EdgeInsets.all(32),
            child: Text(context.t(q.isEmpty ? 'newChat.noContacts' : 'chats.noResults'), textAlign: TextAlign.center, style: ts(14, color: t.muted)),
          ),
        for (final c in contacts) ContactRow(c: c, onTap: () => openDirect(c.accountId)),
      ]),
    );
  }
}
