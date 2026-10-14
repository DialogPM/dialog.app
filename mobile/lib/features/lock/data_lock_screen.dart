// The local database is encrypted with a key that only the account password unlocks.
// - unlock (app start): nothing on disk is readable until the password is entered.
// - protect: the data is not locked yet (just linked, or from an older version).

import 'dart:async';

import 'package:flutter/material.dart';

import '../../app/globals.dart';
import '../../core/backend.dart';
import '../../l10n/i18n.dart';
import '../../state/actions.dart';
import '../../theme/app_theme.dart';
import '../../widgets/app_icon.dart';
import '../../widgets/dialogs.dart';
import '../../widgets/ui.dart';
import '../settings/logout.dart';

class DataLockScreen extends StatefulWidget {
  const DataLockScreen({super.key, required this.protect});

  final bool protect;

  @override
  State<DataLockScreen> createState() => _DataLockScreenState();
}

class _DataLockScreenState extends State<DataLockScreen> {
  final _password = TextEditingController();
  final _focus = FocusNode();
  bool _busy = false;
  String? _error;

  String get _mode => widget.protect ? 'protect' : 'unlock';

  @override
  void dispose() {
    _password.dispose();
    _focus.dispose();
    super.dispose();
  }

  Future<void> _submit() async {
    if (_password.text.isEmpty) return;
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      if (widget.protect) {
        await backend.protectLocalData(_password.text);
      } else {
        await backend.unlock(_password.text);
      }
      final s = await backend.getState();
      // The password was just entered: the app lock must not ask for it again now.
      initAppLock(s.account?.accountId, atStartup: false);
      store.set((d) => d.copyWith(appState: s));
      if (!widget.protect && s.ready) await loadSession();
    } catch (e) {
      if (!mounted) return;
      final wrong = parseError(e).code == 'invalid_password';
      setState(() => _error = wrong ? tr(widget.protect ? 'lock.wrongPassword' : 'dataLock.wrongPassword') : errorText(e));
      _focus.requestFocus();
      _password.selection = TextSelection(baseOffset: 0, extentOffset: _password.text.length);
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _reset() async {
    final ok = await showConfirm(
      context,
      title: context.t('dataLock.resetTitle'),
      body: context.t('dataLock.resetBody'),
      confirmLabel: context.t('dataLock.reset'),
    );
    if (ok) await attempt(backend.resetLocalData);
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    return PopScope(
      canPop: false,
      child: Scaffold(
        backgroundColor: t.canvas,
        body: SafeArea(
          child: Center(
            child: SingleChildScrollView(
              padding: const EdgeInsets.all(22),
              child: ConstrainedBox(
                constraints: const BoxConstraints(maxWidth: 400),
                child: Container(
                  padding: const EdgeInsets.all(24),
                  decoration: BoxDecoration(color: t.pane, borderRadius: BorderRadius.circular(t.rModal), border: Border.all(color: t.line)),
                  child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, mainAxisSize: MainAxisSize.min, children: [
                    Center(
                      child: Container(
                        width: 64,
                        height: 64,
                        decoration: BoxDecoration(color: t.accent, borderRadius: BorderRadius.circular(19)),
                        child: Center(child: AppIcon('lock', size: 28, color: t.onAccent)),
                      ),
                    ),
                    const SizedBox(height: 18),
                    Text(context.t('dataLock.${_mode}Title'), textAlign: TextAlign.center, style: ts(24, weight: 700, color: t.ink, letterSpacing: -.4)),
                    const SizedBox(height: 6),
                    Text(context.t('dataLock.${_mode}Lead'), textAlign: TextAlign.center, style: ts(14.5, color: t.muted, height: 1.5)),
                    const SizedBox(height: 20),
                    AppField(
                      controller: _password,
                      focusNode: _focus,
                      label: context.t('password.label'),
                      password: true,
                      large: true,
                      autofocus: true,
                      error: _error,
                      onChanged: (_) => setState(() {}),
                      onSubmitted: (_) => _submit(),
                    ),
                    const SizedBox(height: 18),
                    Btn(
                      label: context.t(widget.protect ? 'dataLock.protect' : 'lock.unlock'),
                      busy: _busy,
                      onPressed: _password.text.isEmpty ? null : _submit,
                    ),
                    const SizedBox(height: 14),
                    LinkBtn(
                      label: context.t(widget.protect ? 'lock.forgot' : 'dataLock.forgot'),
                      color: t.muted,
                      fontSize: 13,
                      weight: 500,
                      onPressed: () => widget.protect ? confirmLogout(context) : _reset(),
                    ),
                  ]),
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }
}

/// The password was changed on another device: asks for the new one so the local lock
/// uses it too ("Later" keeps the old one for now).
Future<void> promptLocalPasswordUpdate(BuildContext context) async {
  final pw = TextEditingController();
  await showAppDialog<void>(context, title: context.t('dataLock.outdatedTitle'), builder: (ctx) {
    String? error;
    var busy = false;
    return StatefulBuilder(builder: (ctx, setState) {
      Future<void> submit() async {
        setState(() {
          busy = true;
          error = null;
        });
        try {
          await backend.protectLocalData(pw.text);
          store.set((d) => d.copyWith(details: d.details?.copyWith(localPasswordOutdated: false)));
          if (ctx.mounted) Navigator.of(ctx).pop();
          notifyOk(tr('dataLock.updated'));
        } catch (e) {
          setState(() => error = parseError(e).code == 'invalid_password' ? tr('lock.wrongPassword') : errorText(e));
        } finally {
          if (ctx.mounted) setState(() => busy = false);
        }
      }

      return Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
        Text(ctx.t('dataLock.outdatedLead'), style: ts(13.5, color: ctx.tk.muted, height: 1.45)),
        const SizedBox(height: 14),
        AppField(
          controller: pw,
          label: ctx.t('dataLock.newPassword'),
          password: true,
          autofocus: true,
          error: error,
          onChanged: (_) => setState(() {}),
          onSubmitted: (_) => pw.text.isEmpty ? null : submit(),
        ),
        const SizedBox(height: 18),
        DialogActions(children: [
          Btn(label: ctx.t('dataLock.later'), kind: BtnKind.secondary, height: 42, expand: false, onPressed: () => Navigator.of(ctx).pop()),
          Btn(label: ctx.t('dataLock.update'), height: 42, expand: false, busy: busy, onPressed: pw.text.isEmpty ? null : submit),
        ]),
      ]);
    });
  });
  pw.dispose();
}
