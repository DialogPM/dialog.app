// First-run setup, right after creating an account (port of
// app/src/features/onboarding/SetupScreen.tsx): the name others see, an optional username and
// the look of the app. Every step can be skipped; the account works without them.

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../../app/globals.dart';
import '../../l10n/i18n.dart';
import '../../state/actions.dart' show errorText;
import '../../theme/app_theme.dart';
import '../../widgets/ui.dart';
import '../settings/settings_pages.dart' show AppearanceSection;
import 'onboarding.dart' show OnboardingTopBar;

const _steps = ['name', 'username', 'look'];

/// Validation message key for a username (without "@"), or null when valid.
String? _usernameProblem(String u) {
  if (u.isEmpty) return null;
  if (!RegExp(r'^[a-z0-9_.]*$').hasMatch(u)) return 'settings.username.errChars';
  if (!RegExp(r'^[a-z]').hasMatch(u)) return 'settings.username.errStart';
  if (u.length < 3) return 'settings.username.errShort';
  if (u.length > 32) return 'settings.username.errLong';
  final d = RegExp(r'\.([0-9]+)$').firstMatch(u)?.group(1);
  if (d != null && (d.length < 2 || d.length > 9 || !RegExp(r'[1-9]').hasMatch(d) || (d.length > 2 && d.startsWith('0')))) {
    return 'settings.username.errNumber';
  }
  return null;
}

class SetupScreen extends StatefulWidget {
  const SetupScreen({super.key, required this.accountId});
  final String accountId;

  @override
  State<SetupScreen> createState() => _SetupScreenState();
}

class _SetupScreenState extends State<SetupScreen> {
  int _step = 0;
  late final _name = TextEditingController(text: appData.profile?.name ?? '');
  final _username = TextEditingController();
  bool _busy = false;
  String? _error;

  @override
  void dispose() {
    _name.dispose();
    _username.dispose();
    super.dispose();
  }

  void _next() {
    setState(() => _error = null);
    if (_step < _steps.length - 1) {
      setState(() => _step++);
    } else {
      prefsNotifier.update((p) => p.copyWith(setupPending: {...p.setupPending}..remove(widget.accountId)));
    }
  }

  Future<void> _run(Future<void> Function() save) async {
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await save();
      _next();
    } catch (e) {
      setState(() => _error = errorText(e));
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _primary() async {
    switch (_steps[_step]) {
      case 'name':
        final value = _name.text.trim();
        if (value.isEmpty) return _next();
        await _run(() async {
          final p = await backend.setProfile(name: value);
          store.set((d) => d.copyWith(profile: p));
        });
      case 'username':
        final u = _username.text;
        if (u.isEmpty) return _next();
        if (_usernameProblem(u) != null) return;
        await _run(() async => backend.setUsername(u));
      default:
        _next();
    }
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    final kind = _steps[_step];
    final problem = kind == 'username' ? _usernameProblem(_username.text) : null;
    return PopScope(
      canPop: false,
      onPopInvokedWithResult: (didPop, _) {
        if (!didPop && _step > 0) setState(() => _step--);
      },
      child: Scaffold(
        backgroundColor: t.canvas,
        body: SafeArea(
          child: SingleChildScrollView(
            padding: const EdgeInsets.fromLTRB(22, 16, 22, 28),
            child: Center(
              child: ConstrainedBox(
                constraints: const BoxConstraints(maxWidth: 440),
                child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
                  const OnboardingTopBar(steps: 2),
                  const SizedBox(height: 28),
                  Text(context.t('setup.progress', args: {'current': _step + 1, 'total': _steps.length}),
                      style: ts(13.5, weight: 700, color: t.accentText)),
                  const SizedBox(height: 6),
                  Text(context.t('setup.$kind.title'), style: ts(27, weight: 700, color: t.ink, letterSpacing: -0.54)),
                  const SizedBox(height: 6),
                  Text(context.t('setup.$kind.lead'), style: ts(14.5, color: t.muted, height: 1.5)),
                  const SizedBox(height: 20),
                  if (kind == 'name')
                    AppField(
                      key: const ValueKey('name'),
                      controller: _name,
                      label: context.t('setup.name.label'),
                      hint: context.t('setup.name.placeholder'),
                      autofocus: true,
                      large: true,
                      maxLength: 64,
                      error: _error,
                      textInputAction: TextInputAction.next,
                      onSubmitted: (_) => _primary(),
                    ),
                  if (kind == 'username')
                    AppField(
                      key: const ValueKey('username'),
                      controller: _username,
                      label: context.t('setup.username.label'),
                      hint: '@${context.t('setup.username.placeholder')}',
                      description: context.t('setup.username.hint'),
                      autofocus: true,
                      large: true,
                      maxLength: 32,
                      keyboardType: TextInputType.visiblePassword,
                      inputFormatters: [
                        TextInputFormatter.withFunction((_, v) => v.copyWith(text: v.text.toLowerCase().replaceAll(RegExp(r'[@\s]'), ''))),
                      ],
                      error: _error ?? (problem != null ? context.t(problem) : null),
                      onChanged: (_) => setState(() {}),
                      onSubmitted: (_) => _primary(),
                    ),
                  if (kind == 'look') const AppearanceSection(embedded: true),
                  const SizedBox(height: 20),
                  Btn(
                    label: _busy ? context.t('setup.saving') : context.t(kind == 'look' ? 'setup.start' : 'setup.continue'),
                    busy: _busy,
                    onPressed: problem != null ? null : _primary,
                  ),
                  if (kind != 'look') ...[
                    const SizedBox(height: 14),
                    Center(child: LinkBtn(label: context.t('setup.skip'), onPressed: _busy ? null : _next)),
                  ],
                ]),
              ),
            ),
          ),
        ),
      ),
    );
  }
}
