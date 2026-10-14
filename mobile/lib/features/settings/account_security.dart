// Account security prompts over the home screen: a banner when a new device joined the account
// (until dismissed) and approval of new password logins (when the account requires it).
// Driven by the core events device_alerts and login_requests (crates/chat-core/src/engine/security.rs).

import 'dart:async';

import 'package:flutter/material.dart';

import '../../app/globals.dart';
import '../../app/nav.dart';
import '../../core/models.dart';
import '../../l10n/i18n.dart';
import '../../state/actions.dart';
import '../../theme/app_theme.dart';
import '../../util/format.dart';
import '../../widgets/app_icon.dart';
import '../../widgets/dialogs.dart';
import '../../widgets/ui.dart';

/// "New device connected" banner; stays until dismissed or reviewed.
class NewDeviceBanner extends StatefulWidget {
  const NewDeviceBanner({super.key});

  @override
  State<NewDeviceBanner> createState() => _NewDeviceBannerState();
}

class _NewDeviceBannerState extends State<NewDeviceBanner> {
  List<NewDeviceAlert> _alerts = const [];
  StreamSubscription<List<NewDeviceAlert>>? _sub;

  @override
  void initState() {
    super.initState();
    _sub = backend.onDeviceAlerts.listen((list) {
      if (mounted) setState(() => _alerts = list);
    });
    unawaited(backend.deviceAlerts().then((list) {
      if (mounted) setState(() => _alerts = list);
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
    final last = _alerts.last;
    final more = _alerts.length > 1 ? ' (+${_alerts.length - 1})' : '';
    final body = context.t('security.newDeviceBody',
        args: {'name': last.name, 'platform': last.platform, 'date': formatDateTime(last.createdAt, context.lang)});
    return Container(
      color: Color.alphaBlend(t.danger.withValues(alpha: .12), t.pane),
      padding: const EdgeInsets.fromLTRB(14, 10, 14, 10),
      child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
        Row(children: [
          AppIcon('alert', size: 16, color: t.danger),
          const SizedBox(width: 8),
          Expanded(
            child: Text('${context.t('security.newDeviceTitle')}: $body$more',
                style: ts(13, weight: 600, color: t.danger)),
          ),
        ]),
        const SizedBox(height: 4),
        Text(context.t('security.newDeviceHint'), style: ts(12.5, color: t.danger)),
        const SizedBox(height: 8),
        Wrap(spacing: 8, runSpacing: 8, children: [
          ChipBtn(label: context.t('security.review'), tone: 'danger', onPressed: () => openSettingsSection('devices')),
          ChipBtn(label: context.t('security.dismiss'), tone: 'pane', onPressed: () => attempt(() => backend.dismissDeviceAlerts())),
        ]),
      ]),
    );
  }
}

/// Asks to approve or deny logins waiting for approval (invisible otherwise).
class LoginRequestWatcher extends StatefulWidget {
  const LoginRequestWatcher({super.key});

  @override
  State<LoginRequestWatcher> createState() => _LoginRequestWatcherState();
}

class _LoginRequestWatcherState extends State<LoginRequestWatcher> {
  StreamSubscription<List<LoginRequestInfo>>? _sub;
  final _asked = <String>{};
  bool _showing = false;

  @override
  void initState() {
    super.initState();
    _sub = backend.onLoginRequests.listen(_handle);
    unawaited(backend.listLoginRequests().then(_handle, onError: (_) {}));
  }

  @override
  void dispose() {
    _sub?.cancel();
    super.dispose();
  }

  Future<void> _handle(List<LoginRequestInfo> list) async {
    if (!mounted || _showing) return;
    final now = DateTime.now().millisecondsSinceEpoch;
    final next = list.where((r) => r.expiresAt > now && !_asked.contains(r.id)).firstOrNull;
    if (next == null) return;
    _asked.add(next.id);
    _showing = true;
    final approve = await showAppDialog<bool>(
      context,
      title: context.t('security.loginRequestTitle'),
      builder: (ctx) => Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
        Text(
          ctx.t('security.loginRequestBody',
              args: {'name': next.name, 'platform': next.platform, 'date': formatDateTime(next.createdAt, ctx.lang)}),
          style: ts(14, color: ctx.tk.muted, height: 1.45),
        ),
        const SizedBox(height: 18),
        DialogActions(children: [
          Btn(label: ctx.t('security.approve'), kind: BtnKind.secondary, height: 42, expand: false, onPressed: () => Navigator.of(ctx).pop(true)),
          Btn(label: ctx.t('security.deny'), kind: BtnKind.danger, height: 42, expand: false, onPressed: () => Navigator.of(ctx).pop(false)),
        ]),
      ]),
    );
    _showing = false;
    // Dismissing the dialog without choosing leaves the request pending (it expires).
    if (approve != null) await attempt(() => backend.answerLoginRequest(next.id, approve));
    if (mounted) unawaited(backend.listLoginRequests().then(_handle, onError: (_) {}));
  }

  @override
  Widget build(BuildContext context) => const SizedBox.shrink();
}
