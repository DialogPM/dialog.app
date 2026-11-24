// Notices from the operator of the server the account is on (state/notices.dart): newest first,
// read as soon as the page opens, deleted one by one (swipe or the menu). They are signed by the
// operator and checked by the core, and they are not chat messages: there is nobody to reply
// to, so the page has no composer and says so at the top. The text is plain: shown exactly as
// written with its line breaks, never through the message rich-text parser (no markdown, no
// links), so a notice cannot dress itself up.

import 'dart:async';

import 'package:flutter/material.dart';

import '../../core/models.dart';
import '../../l10n/i18n.dart';
import '../../services/notifications.dart';
import '../../state/actions.dart' show notifyError, notifyOk;
import '../../state/notices.dart';
import '../../theme/app_theme.dart';
import '../../util/format.dart';
import '../../widgets/app_icon.dart';
import '../../widgets/dialogs.dart';
import '../../widgets/ui.dart';

class NoticesPage extends StatefulWidget {
  const NoticesPage({super.key});

  @override
  State<NoticesPage> createState() => _NoticesPageState();
}

class _NoticesPageState extends State<NoticesPage> {
  /// Notices that were unread when the page opened: they keep a "new" mark while it is open,
  /// although they are marked read right away.
  late final Set<int> _fresh = {for (final n in noticesState.value.notices) if (!n.read) n.id};

  @override
  void initState() {
    super.initState();
    noticesPageOpen = true;
    unawaited(Notifications.instance.cancelNotices());
    unawaited(_refresh());
  }

  Future<void> _refresh() async {
    await loadNotices();
    if (!mounted) return;
    setState(() => _fresh.addAll([for (final n in noticesState.value.notices) if (!n.read) n.id]));
    await markNoticesRead();
  }

  @override
  void dispose() {
    noticesPageOpen = false;
    super.dispose();
  }

  Future<void> _delete(Notice n) async {
    try {
      await deleteNotice(n.id);
      notifyOk(tr('mobile.notices.deleted'));
    } catch (e) {
      notifyError(e);
    }
  }

  void _menu(Notice n) => showActionSheet(context, title: n.title.isEmpty ? null : n.title, actions: [
        SheetAction(label: context.t('mobile.notices.delete'), icon: 'trash', danger: true, onTap: () => unawaited(_delete(n))),
      ]);

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    return PaneScaffold(
      header: PaneHeader(title: context.t('mobile.notices.title')),
      body: ValueListenableBuilder<NoticesState>(
        valueListenable: noticesState,
        builder: (context, s, _) => ListView(padding: const EdgeInsets.only(top: 14, bottom: 20), children: [
          Padding(
            padding: const EdgeInsets.fromLTRB(16, 0, 16, 14),
            child: NoticeBox(icon: 'server', text: context.t('mobile.notices.header', args: {'server': noticeServerName()})),
          ),
          if (s.notices.isEmpty)
            Padding(
              padding: const EdgeInsets.fromLTRB(28, 36, 28, 24),
              child: Column(children: [
                AppIcon('bell', size: 30, color: t.faint),
                const SizedBox(height: 12),
                Text(context.t('mobile.notices.empty'), textAlign: TextAlign.center, style: ts(14, color: t.muted, height: 1.45)),
              ]),
            ),
          for (final n in s.notices)
            Dismissible(
              key: ValueKey('notice-${n.id}'),
              direction: DismissDirection.endToStart,
              background: Container(
                alignment: AlignmentDirectional.centerEnd,
                padding: const EdgeInsetsDirectional.only(end: 28),
                color: t.danger,
                child: AppIcon('trash', size: 20, color: t.onAccent, semanticLabel: context.t('mobile.notices.delete')),
              ),
              onDismissed: (_) => unawaited(_delete(n)),
              child: _NoticeCard(notice: n, fresh: _fresh.contains(n.id), onMenu: () => _menu(n)),
            ),
        ]),
      ),
    );
  }
}

class _NoticeCard extends StatelessWidget {
  const _NoticeCard({required this.notice, required this.fresh, required this.onMenu});

  final Notice notice;
  final bool fresh;
  final VoidCallback onMenu;

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    final n = notice;
    return Sect(
      padding: const EdgeInsetsDirectional.fromSTEB(16, 12, 6, 14),
      child: InkWell(
        onLongPress: onMenu,
        child: Row(crossAxisAlignment: CrossAxisAlignment.start, children: [
          Expanded(
            child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
              Row(children: [
                if (fresh) ...[
                  Container(
                    width: 8,
                    height: 8,
                    decoration: BoxDecoration(color: t.accent, shape: BoxShape.circle),
                  ),
                  const SizedBox(width: 7),
                ],
                Expanded(
                  child: Text(formatDateTime(n.createdAt, context.lang), style: ts(12, weight: 600, color: fresh ? t.accentText : t.faint)),
                ),
              ]),
              if (n.title.isNotEmpty) ...[
                const SizedBox(height: 4),
                Text(n.title, style: ts(15.5, weight: 700, color: t.ink, height: 1.3)),
              ],
              if (n.body.isNotEmpty) ...[
                const SizedBox(height: 6),
                // Plain text on purpose (see the top of this file).
                SelectableText(n.body, style: ts(14, color: t.ink, height: 1.5)),
              ],
            ]),
          ),
          IconBtn('more', size: 36, iconSize: 18, color: t.muted, tooltip: context.t('common.more'), onPressed: onMenu),
        ]),
      ),
    );
  }
}
