// Starred messages of every chat (this device only, a week each).

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../app/globals.dart';
import '../../app/nav.dart';
import '../../core/models.dart';
import '../../l10n/i18n.dart';
import '../../state/actions.dart';
import '../../state/store.dart';
import '../../theme/app_theme.dart';
import '../../widgets/ui.dart';
import 'chat_list.dart';

class StarredPage extends ConsumerStatefulWidget {
  const StarredPage({super.key});

  @override
  ConsumerState<StarredPage> createState() => _StarredPageState();
}

class _StarredPageState extends ConsumerState<StarredPage> {
  List<Message>? _list;

  @override
  void initState() {
    super.initState();
    _load();
  }

  Future<void> _load() async {
    try {
      final list = await backend.starredMessages();
      if (mounted) setState(() => _list = list);
    } catch (_) {
      if (mounted) setState(() => _list = const []);
    }
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    final convs = ref.watch(storeProvider.select((s) => s.conversations));
    final list = (_list ?? const <Message>[]).where((m) => convs.containsKey(m.conversationId)).toList();
    return Scaffold(
      backgroundColor: t.pane,
      body: SafeArea(
        child: Column(children: [
          PaneHeader(title: context.t('star.title')),
          Expanded(
            child: _list != null && list.isEmpty
                ? Padding(
                    padding: const EdgeInsets.all(32),
                    child: Column(children: [
                      Text(context.t('star.empty'), style: ts(15, weight: 650, color: t.ink)),
                      const SizedBox(height: 6),
                      Text(context.t('star.emptyHint'), textAlign: TextAlign.center, style: ts(13.5, color: t.muted)),
                    ]),
                  )
                : ListView(children: [
                    for (final m in list)
                      Row(children: [
                        Expanded(
                          child: ChatListItem(
                            conv: convs[m.conversationId]!,
                            showPreviewOf: (m, ''),
                            onTap: () => openChat(m.conversationId, highlightId: m.id),
                          ),
                        ),
                        IconBtn(
                          'star',
                          color: const Color(0xFFE8A400),
                          tooltip: context.t('star.unstar'),
                          onPressed: () async {
                            await starMessage(m, false);
                            await _load();
                          },
                        ),
                      ]),
                  ]),
          ),
        ]),
      ),
    );
  }
}

/// Opens the starred messages page.
void openStarred(BuildContext context) => Navigator.of(context).push(MaterialPageRoute<void>(builder: (_) => const StarredPage()));
