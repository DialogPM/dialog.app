// Full-screen lock shown when the app lock is enabled (app start, or back from the background
// after 5 minutes). Unlocks with biometrics when available, or with the account password.

import 'dart:async';

import 'package:flutter/material.dart';
import 'package:local_auth/local_auth.dart';

import '../../app/globals.dart';
import '../../l10n/i18n.dart';
import '../../state/actions.dart';
import '../../theme/app_theme.dart';
import '../../widgets/app_icon.dart';
import '../../widgets/ui.dart';
import '../settings/logout.dart';

class LockScreen extends StatefulWidget {
  const LockScreen({super.key});

  @override
  State<LockScreen> createState() => _LockScreenState();
}

class _LockScreenState extends State<LockScreen> {
  final _password = TextEditingController();
  final _focus = FocusNode();
  final _auth = LocalAuthentication();
  bool _busy = false;
  String? _error;
  bool _biometrics = false;

  @override
  void initState() {
    super.initState();
    if (isMobilePlatform) unawaited(_initBiometrics());
  }

  @override
  void dispose() {
    _password.dispose();
    _focus.dispose();
    super.dispose();
  }

  Future<void> _initBiometrics() async {
    try {
      final ok = await _auth.isDeviceSupported() && (await _auth.getAvailableBiometrics()).isNotEmpty;
      if (!mounted) return;
      setState(() => _biometrics = ok);
      if (ok) await _biometric();
    } catch (_) {}
  }

  Future<void> _biometric() async {
    try {
      final ok = await _auth.authenticate(localizedReason: tr('mobile.biometricReason'), biometricOnly: true);
      if (ok) unlockApp();
    } catch (_) {}
  }

  Future<void> _submit() async {
    if (_password.text.isEmpty) return;
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      if (await backend.verifyPassword(_password.text)) {
        unlockApp();
        return;
      }
      setState(() => _error = tr('lock.wrongPassword'));
      // Keep the keyboard open and select the wrong password so it can be retyped.
      _focus.requestFocus();
      _password.selection = TextSelection(baseOffset: 0, extentOffset: _password.text.length);
    } catch (e) {
      setState(() => _error = errorText(e));
    } finally {
      if (mounted) setState(() => _busy = false);
    }
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
                    Text(context.t('lock.locked'), textAlign: TextAlign.center, style: ts(24, weight: 700, color: t.ink, letterSpacing: -.4)),
                    const SizedBox(height: 6),
                    Text(context.t('lock.lead'), textAlign: TextAlign.center, style: ts(14.5, color: t.muted, height: 1.5)),
                    const SizedBox(height: 20),
                    AppField(
                      controller: _password,
                      focusNode: _focus,
                      label: context.t('password.label'),
                      password: true,
                      large: true,
                      autofocus: !_biometrics,
                      error: _error,
                      onChanged: (_) => setState(() {}),
                      onSubmitted: (_) => _submit(),
                    ),
                    const SizedBox(height: 18),
                    Btn(label: context.t('lock.unlock'), busy: _busy, onPressed: _password.text.isEmpty ? null : _submit),
                    if (_biometrics) ...[
                      const SizedBox(height: 10),
                      Btn(label: context.t('mobile.useBiometrics'), icon: 'fingerprint', kind: BtnKind.secondary, onPressed: _biometric),
                    ],
                    const SizedBox(height: 14),
                    LinkBtn(label: context.t('lock.forgot'), color: t.muted, fontSize: 13, weight: 500, onPressed: () => confirmLogout(context)),
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
