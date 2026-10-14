// Key transparency (docs/KEY-TRANSPARENCY.md): a banner over the home screen when the core
// detects that the server's public key log is inconsistent (event kt_alerts), and the status of
// a contact's key on the safety number page.

import 'dart:async';

import 'package:flutter/material.dart';

import '../../app/globals.dart';
import '../../core/models.dart';
import '../../l10n/i18n.dart';
import '../../state/actions.dart';
import '../../theme/app_theme.dart';
import '../../widgets/app_icon.dart';
import '../../widgets/ui.dart';

/// "Security alert: the server's key log is inconsistent"; stays until dismissed.
class KtAlertBanner extends StatefulWidget {
  const KtAlertBanner({super.key});

  @override
  State<KtAlertBanner> createState() => _KtAlertBannerState();
}

class _KtAlertBannerState extends State<KtAlertBanner> {
  List<KtAlert> _alerts = const [];
  StreamSubscription<List<KtAlert>>? _sub;

  @override
  void initState() {
    super.initState();
    _sub = backend.onKtAlerts.listen((list) {
      if (mounted) setState(() => _alerts = list);
    });
    unawaited(backend.ktStatus().then((o) {
      if (mounted) setState(() => _alerts = o.alerts);
    }, onError: (_) {}));
  }

  @override
  void dispose() {
    _sub?.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    if (_alerts.isEmpty) return const SizedBox.shrink();
    final t = context.tk;
    final more = _alerts.length > 1 ? ' (+${_alerts.length - 1})' : '';
    return Container(
      color: Color.alphaBlend(t.danger.withValues(alpha: .12), t.pane),
      padding: const EdgeInsets.fromLTRB(14, 10, 14, 10),
      child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
        Row(children: [
          AppIcon('alert', size: 16, color: t.danger),
          const SizedBox(width: 8),
          Expanded(
            child: Text('${context.t('kt.alertTitle')}: ${context.t('kt.kind.${_alerts.last.kind}')}$more',
                style: ts(13, weight: 600, color: t.danger)),
          ),
        ]),
        const SizedBox(height: 4),
        Text(context.t('kt.alertHint'), style: ts(12.5, color: t.danger)),
        const SizedBox(height: 8),
        Wrap(spacing: 8, runSpacing: 8, children: [
          ChipBtn(label: context.t('security.dismiss'), tone: 'pane', onPressed: () => attempt(() => backend.ktDismissAlerts())),
        ]),
      ]),
    );
  }
}

/// "Key transparency: verified / not verified yet / ALERT" for a contact's key.
class KtContactNotice extends StatefulWidget {
  const KtContactNotice({super.key, required this.accountId});
  final String accountId;

  @override
  State<KtContactNotice> createState() => _KtContactNoticeState();
}

class _KtContactNoticeState extends State<KtContactNotice> {
  KtContactStatus? _st;

  @override
  void initState() {
    super.initState();
    unawaited(backend.ktContactStatus(widget.accountId).then((s) {
      if (mounted) setState(() => _st = s);
    }, onError: (_) {}));
  }

  @override
  Widget build(BuildContext context) {
    final st = _st;
    if (st == null || st.status == 'unavailable') return const SizedBox.shrink();
    final alert = st.status == 'alert';
    return Padding(
      padding: const EdgeInsets.only(top: 18),
      child: NoticeBox(
        tone: alert ? 'danger' : 'info',
        icon: alert ? 'alert' : (st.status == 'verified' ? 'okcircle' : 'shield'),
        title: '${context.t('kt.title')}: ${context.t('kt.status.${st.status}')}',
        text: context.t('kt.statusHint.${st.status}'),
      ),
    );
  }
}
