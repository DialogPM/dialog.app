// "Remember the password on this phone": opt-in, per account (chat-core local_lock.rs). On, the
// key of the local data is kept on disk and the account opens at startup without the password.
// Turning it on shows the warning once and needs the password as proof. Port of the desktop
// app/src/features/lock/RememberPassword.tsx.

import 'package:flutter/material.dart';

import '../../app/globals.dart';
import '../../core/backend.dart' show parseError;
import '../../l10n/i18n.dart';
import '../../state/actions.dart';
import '../../theme/app_theme.dart';
import '../../widgets/dialogs.dart';
import '../../widgets/ui.dart';

/// The warning (anyone with the files can read the messages; not a replacement for the 24 words).
Future<bool> confirmRememberPassword(BuildContext context) => showConfirm(
      context,
      title: context.t('autoUnlock.warnTitle'),
      body: context.t('autoUnlock.warnBody'),
      confirmLabel: context.t('autoUnlock.warnConfirm'),
    );

/// Checkbox under a password field; checking it shows the warning first.
class RememberPasswordCheck extends StatelessWidget {
  const RememberPasswordCheck({super.key, required this.on, required this.onChanged});

  final bool on;
  final ValueChanged<bool> onChanged;

  Future<void> _toggle(BuildContext context) async {
    if (on) {
      onChanged(false);
      return;
    }
    if (await confirmRememberPassword(context)) onChanged(true);
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    return Semantics(
      checked: on,
      child: GestureDetector(
        behavior: HitTestBehavior.opaque,
        onTap: () => _toggle(context),
        child: Padding(
          padding: const EdgeInsets.symmetric(vertical: 4),
          child: Row(children: [
            CheckBox(on: on),
            const SizedBox(width: 12),
            Expanded(child: Text(context.t('autoUnlock.remember'), style: ts(14, color: t.ink))),
          ]),
        ),
      ),
    );
  }
}

/// Reloads the app state (AppState.autoUnlock) after the setting changed.
Future<void> refreshAutoUnlock() async {
  final s = await backend.getState();
  store.set((d) => d.copyWith(appState: s));
}

/// Settings: the warning, then the account password as proof.
Future<void> enableAutoUnlock(BuildContext context) async {
  if (!await confirmRememberPassword(context) || !context.mounted) return;
  final pw = TextEditingController();
  await showAppDialog<void>(context, title: context.t('autoUnlock.remember'), builder: (ctx) {
    String? error;
    var busy = false;
    return StatefulBuilder(builder: (ctx, setState) {
      Future<void> submit() async {
        if (pw.text.isEmpty || busy) return;
        setState(() {
          busy = true;
          error = null;
        });
        try {
          await backend.setAutoUnlock(enabled: true, password: pw.text);
          await refreshAutoUnlock();
          if (ctx.mounted) Navigator.of(ctx).pop();
          notifyOk(tr('autoUnlock.enabled'));
        } catch (e) {
          setState(() => error = parseError(e).code == 'invalid_password' ? tr('lock.wrongPassword') : errorText(e));
        } finally {
          if (ctx.mounted) setState(() => busy = false);
        }
      }

      return Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
        Text(ctx.t('autoUnlock.enableLead'), style: ts(13.5, color: ctx.tk.muted, height: 1.45)),
        const SizedBox(height: 14),
        AppField(
          controller: pw,
          label: ctx.t('password.label'),
          password: true,
          autofocus: true,
          error: error,
          onChanged: (_) => setState(() {}),
          onSubmitted: (_) => submit(),
        ),
        const SizedBox(height: 18),
        DialogActions(children: [
          Btn(label: ctx.t('common.cancel'), kind: BtnKind.secondary, height: 42, expand: false, onPressed: () => Navigator.of(ctx).pop()),
          Btn(label: ctx.t('autoUnlock.warnConfirm'), height: 42, expand: false, busy: busy, onPressed: pw.text.isEmpty ? null : submit),
        ]),
      ]);
    });
  });
  pw.dispose();
}

/// Back to the default: the password is asked at every start.
Future<void> disableAutoUnlock() async {
  final ok = await attempt(() async {
    await backend.setAutoUnlock(enabled: false);
    await refreshAutoUnlock();
    return true;
  });
  if (ok == true) notifyOk(tr('autoUnlock.disabled'));
}
