// Start screen: the accounts saved on this phone (profile_picker.dart), then welcome (with the
// server choice), create an account (password, then the recovery phrase), log in (password or QR
// from another device), recover with the 24 words, or open a hidden account by its number.

import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../app/globals.dart';
import '../../core/backend.dart' show parseError;
import '../../core/models.dart';
import '../../l10n/i18n.dart';
import '../../state/actions.dart';
import '../../state/prefs.dart';
import '../../state/store.dart';
import '../../theme/app_theme.dart';
import '../../util/format.dart';
import '../../widgets/app_icon.dart';
import '../../widgets/ui.dart';
import '../devices/link_device.dart';
import 'profile_picker.dart';
import '../../services/remote_wipe.dart';

enum _Step { welcome, register, login, recover, other }

class Onboarding extends ConsumerStatefulWidget {
  const Onboarding({super.key, required this.defaultServer, this.resumeAccount, this.resumeMnemonic});

  final String defaultServer;

  /// Resume at the recovery phrase step (the app was closed before confirming it).
  final AccountInfo? resumeAccount;
  final List<String>? resumeMnemonic;

  @override
  ConsumerState<Onboarding> createState() => _OnboardingState();
}

class _OnboardingState extends ConsumerState<Onboarding> {
  late _Step _step = widget.resumeAccount != null ? _Step.register : _Step.welcome;
  late String _serverUrl = widget.resumeAccount?.serverUrl ?? widget.defaultServer;
  ServerInfo? _server;
  late int _regStage = widget.resumeAccount != null ? 1 : 0;
  // Accounts saved on this phone (not while finishing a registration).
  List<ProfileCard> _profiles = const [];

  @override
  void initState() {
    super.initState();
    if (widget.resumeAccount == null) {
      _loadProfiles();
      remoteWipeVersion.addListener(_loadProfiles);
    }
  }

  @override
  void dispose() {
    remoteWipeVersion.removeListener(_loadProfiles);
    super.dispose();
  }

  Future<void> _loadProfiles() async {
    try {
      final list = await backend.listProfiles();
      if (mounted) setState(() => _profiles = list);
    } catch (e) {
      debugPrint('profiles: $e');
    }
  }

  void _back() => setState(() => _step = _Step.welcome);

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    Widget content;
    switch (_step) {
      case _Step.welcome:
        final welcome = _Welcome(
          hasProfiles: _profiles.isNotEmpty,
          defaultServer: widget.defaultServer,
          serverUrl: _serverUrl,
          server: _server,
          onServer: (url, info) => setState(() {
            _serverUrl = url;
            _server = info;
          }),
          onPick: (s) => setState(() {
            _regStage = 0;
            _step = s;
          }),
        );
        content = _profiles.isEmpty
            ? welcome
            : Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
                ProfileList(key: ValueKey(_profiles.map((p) => p.id).join(',')), profiles: _profiles, onChanged: _loadProfiles),
                _gap(28),
                welcome,
              ]);
      case _Step.other:
        content = OpenOtherProfile(onBack: _back);
      case _Step.register:
        content = _RegisterFlow(
          serverUrl: _serverUrl,
          onBack: _back,
          resumeAccount: widget.resumeAccount,
          resumeMnemonic: widget.resumeMnemonic,
          onStage: (s) => setState(() => _regStage = s),
        );
      case _Step.login:
        content = _LoginForm(serverUrl: _serverUrl, onBack: _back, onRecover: () => setState(() => _step = _Step.recover));
      case _Step.recover:
        content = _RecoverForm(serverUrl: _serverUrl, onBack: () => setState(() => _step = _Step.login));
    }
    return PopScope(
      canPop: _step == _Step.welcome || widget.resumeAccount != null,
      onPopInvokedWithResult: (didPop, _) {
        if (!didPop) setState(() => _step = _step == _Step.recover ? _Step.login : _Step.welcome);
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
                  OnboardingTopBar(steps: _step == _Step.register ? _regStage : null),
                  const SizedBox(height: 22),
                  const _MobileBrand(),
                  const _LoggedOutNotice(),
                  const SizedBox(height: 22),
                  content,
                ]),
              ),
            ),
          ),
        ),
      ),
    );
  }
}

// ---------------------------------------------------------------------------
// top bar, brand, notices
// ---------------------------------------------------------------------------

class OnboardingTopBar extends ConsumerWidget {
  const OnboardingTopBar({super.key, this.steps});
  final int? steps;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final t = context.tk;
    Widget square({required Widget child, required VoidCallback onTap, required String label}) => Tooltip(
          message: label,
          child: Container(
            width: 36,
            height: 36,
            decoration: BoxDecoration(color: t.pane, borderRadius: BorderRadius.circular(10), border: Border.all(color: t.line)),
            child: Tap(onTap: onTap, radius: BorderRadius.circular(10), semanticLabel: label, child: Center(child: child)),
          ),
        );
    return Row(children: [
      if (steps != null) _Steps(current: steps!),
      const Spacer(),
      Builder(
        builder: (bctx) => square(
          label: context.t('settings.language.title'),
          onTap: () async {
            final box = bctx.findRenderObject() as RenderBox;
            final pos = box.localToGlobal(Offset(0, box.size.height + 4));
            final code = await showMenu<String>(
              context: bctx,
              position: RelativeRect.fromLTRB(pos.dx - 130, pos.dy, pos.dx + 36, 0),
              items: [
                for (final l in kLanguages)
                  PopupMenuItem(
                    value: l.code,
                    child: Row(children: [
                      Expanded(child: Text(l.label)),
                      if (context.lang == l.code) AppIcon('check', size: 16, color: t.accentText),
                    ]),
                  ),
              ],
            );
            if (code != null) ref.read(prefsProvider.notifier).setLanguage(code);
          },
          child: Text(context.lang.toUpperCase(), style: ts(12.5, weight: 700, color: t.muted)),
        ),
      ),
      const SizedBox(width: 8),
      square(
        label: context.t('settings.appearance.toggle'),
        onTap: () => ref.read(prefsProvider.notifier).setTheme(t.dark ? 'light' : 'dark'),
        child: AppIcon(t.dark ? 'sun' : 'moon', size: 17, color: t.muted),
      ),
    ]);
  }
}

/// "1 Account — 2 Recovery" indicator of the registration screens (mobile: only the current label).
class _Steps extends StatelessWidget {
  const _Steps({required this.current});
  final int current;

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    final labels = [context.t('onboarding.steps.account'), context.t('onboarding.steps.recovery'), context.t('onboarding.steps.profile')];
    return Row(children: [
      for (var i = 0; i < labels.length; i++) ...[
        Container(
          width: 24,
          height: 24,
          decoration: BoxDecoration(shape: BoxShape.circle, color: i <= current ? t.accent : t.sunk),
          child: Center(
            child: i < current
                ? AppIcon('check', size: 13, strokeWidth: 2.6, color: t.onAccent)
                : Text('${i + 1}', style: ts(12, weight: 700, color: i <= current ? t.onAccent : t.muted)),
          ),
        ),
        if (i == current) ...[const SizedBox(width: 8), Text(labels[i], style: ts(13, weight: 600, color: t.ink))],
        if (i < labels.length - 1) ...[
          const SizedBox(width: 8),
          Container(width: 18, height: 1.5, color: t.line),
          const SizedBox(width: 8),
        ],
      ],
    ]);
  }
}

class _MobileBrand extends StatelessWidget {
  const _MobileBrand();

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    return Padding(
      padding: const EdgeInsets.only(top: 6),
      child: Row(children: [
        Container(
          width: 44,
          height: 44,
          decoration: BoxDecoration(color: t.accent, borderRadius: BorderRadius.circular(13)),
          child: Center(child: AppIcon('chat', size: 24, color: t.onAccent)),
        ),
        const SizedBox(width: 12),
        Expanded(
          child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
            Text(kBrandName, style: ts(19, weight: 700, color: t.ink)),
            Text(context.t('onboarding.tagline'), style: ts(13, color: t.muted)),
          ]),
        ),
      ]),
    );
  }
}

class _LoggedOutNotice extends ConsumerWidget {
  const _LoggedOutNotice();

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final reason = ref.watch(storeProvider.select((s) => s.loggedOutReason));
    if (reason == null) return const SizedBox.shrink();
    return Padding(
      padding: const EdgeInsets.only(top: 22),
      child: NoticeBox(
        tone: 'warn',
        icon: 'info',
        title: context.t('onboarding.loggedOut.title'),
        text: context.t('onboarding.loggedOut.$reason'),
        onClose: () => store.set((d) => d.copyWith(loggedOutReason: null)),
      ),
    );
  }
}

class _Heading extends StatelessWidget {
  const _Heading(this.title, this.lead);
  final String title;
  final String lead;

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    return Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
      Text(title, style: ts(27, weight: 700, color: t.ink, letterSpacing: -0.54)),
      const SizedBox(height: 6),
      Text(lead, style: ts(14.5, color: t.muted, height: 1.5)),
    ]);
  }
}

Widget _gap([double h = 18]) => SizedBox(height: h);

Widget _errorText(BuildContext context, String? error) =>
    error == null ? const SizedBox.shrink() : Text(error, style: ts(13, color: context.tk.danger, height: 1.45));

// ---------------------------------------------------------------------------
// welcome + server choice
// ---------------------------------------------------------------------------

String normalizeServerUrl(String raw) {
  var u = raw.trim();
  if (u.isNotEmpty && !RegExp(r'^https?://', caseSensitive: false).hasMatch(u)) u = 'https://$u';
  return u.replaceAll(RegExp(r'/+$'), '');
}

class _Welcome extends StatefulWidget {
  const _Welcome({
    required this.hasProfiles,
    required this.defaultServer,
    required this.serverUrl,
    required this.server,
    required this.onServer,
    required this.onPick,
  });

  /// Accounts are saved on this phone: they are listed above, this part adds another one.
  final bool hasProfiles;
  final String defaultServer;
  final String serverUrl;
  final ServerInfo? server;
  final void Function(String url, ServerInfo? info) onServer;
  final ValueChanged<_Step> onPick;

  @override
  State<_Welcome> createState() => _WelcomeState();
}

class _WelcomeState extends State<_Welcome> {
  bool _editing = false;
  late final _url = TextEditingController(text: widget.serverUrl != widget.defaultServer ? widget.serverUrl : '');
  bool _testing = false;
  ServerInfo? _tested;
  String? _testError;
  String? _checkError;

  bool get _custom => widget.serverUrl != widget.defaultServer;

  @override
  void initState() {
    super.initState();
    if (_custom) _tested = widget.server;
    _check();
  }

  @override
  void dispose() {
    _url.dispose();
    super.dispose();
  }

  // Check the chosen server once (tells whether it accepts new accounts).
  void _check() {
    if (widget.server != null || widget.serverUrl.isEmpty) return;
    final url = widget.serverUrl;
    backend.checkServer(url).then((info) {
      if (mounted) widget.onServer(url, info);
    }).catchError((Object e) {
      if (mounted) setState(() => _checkError = errorText(e));
    });
  }

  Future<void> _test() async {
    final u = normalizeServerUrl(_url.text);
    if (u.isEmpty) return;
    setState(() {
      _testing = true;
      _testError = null;
      _tested = null;
    });
    try {
      final info = await backend.checkServer(u);
      if (!mounted) return;
      setState(() {
        _tested = info;
        _checkError = null;
      });
      widget.onServer(info.url.isNotEmpty ? info.url : u, info);
    } catch (e) {
      if (mounted) setState(() => _testError = errorText(e));
    } finally {
      if (mounted) setState(() => _testing = false);
    }
  }

  void _reset() {
    setState(() {
      _editing = false;
      _url.clear();
      _tested = null;
      _testError = null;
      _checkError = null;
    });
    widget.onServer(widget.defaultServer, null);
    WidgetsBinding.instance.addPostFrameCallback((_) => _check());
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    final closed = widget.server != null && !widget.server!.registrationEnabled;
    final dotColor = _checkError != null ? t.danger : (widget.server != null ? t.accent : t.faint);
    final insecure = _custom && widget.serverUrl.toLowerCase().startsWith('http://');
    return Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
      if (widget.hasProfiles)
        Text(context.t('profiles.add'), style: ts(17, weight: 700, color: t.ink))
      else
        _Heading(context.t('onboarding.welcome.title'), context.t('onboarding.welcome.lead')),
      _gap(widget.hasProfiles ? 12 : 18),
      Btn(label: context.t('onboarding.choose.create'), onPressed: closed ? null : () => widget.onPick(_Step.register)),
      if (closed) ...[
        _gap(8),
        Text(context.t('onboarding.server.registrationClosed'), textAlign: TextAlign.center, style: ts(12.5, color: t.danger)),
      ],
      _gap(10),
      Btn(
        label: context.t(widget.hasProfiles ? 'profiles.login' : 'onboarding.choose.login'),
        kind: BtnKind.secondary,
        onPressed: () => widget.onPick(_Step.login),
      ),
      _gap(8),
      Center(child: LinkBtn(label: context.t('mobile.profiles.other'), color: t.muted, fontSize: 13.5, weight: 500, onPressed: () => widget.onPick(_Step.other))),
      _gap(12),
      Divider(height: 1, color: t.line),
      _gap(12),
      Row(children: [
        Container(width: 7, height: 7, decoration: BoxDecoration(shape: BoxShape.circle, color: dotColor)),
        const SizedBox(width: 8),
        Expanded(
          child: Text.rich(
            TextSpan(children: [
              TextSpan(text: '${context.t(_custom ? 'onboarding.server.custom' : 'onboarding.server.default')} '),
              TextSpan(text: widget.serverUrl.isEmpty ? '—' : widget.serverUrl, style: ts(12.5, color: t.muted, mono: true)),
            ]),
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            style: ts(12.5, color: t.muted),
          ),
        ),
        const SizedBox(width: 8),
        LinkBtn(
          label: context.t(_editing ? 'common.close' : 'common.change'),
          color: t.muted,
          fontSize: 12.5,
          underline: true,
          onPressed: () => setState(() => _editing = !_editing),
        ),
      ]),
      if (_checkError != null && !_editing) ...[_gap(8), Text(_checkError!, style: ts(12.5, color: t.danger))],
      if (insecure) ...[_gap(10), NoticeBox(tone: 'warn', icon: 'alert', text: context.t('mobile.httpWarning'))],
      if (_editing) ...[
        _gap(10),
        Container(
          padding: const EdgeInsets.all(14),
          decoration: BoxDecoration(color: t.sunk, borderRadius: BorderRadius.circular(t.rCard)),
          child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
            Text(context.t('onboarding.server.customHint'), style: ts(12.5, color: t.muted, height: 1.45)),
            _gap(8),
            Row(children: [
              Expanded(
                child: SizedBox(
                  height: 40,
                  child: TextField(
                    enableIMEPersonalizedLearning: imePersonalizedLearning,
                    controller: _url,
                    autofocus: true,
                    keyboardType: TextInputType.url,
                    autocorrect: false,
                    enableSuggestions: false,
                    onChanged: (_) => setState(() {
                      _tested = null;
                      _testError = null;
                    }),
                    onSubmitted: (_) => _test(),
                    style: ts(13, color: t.ink, mono: true),
                    decoration: InputDecoration(
                      isDense: true,
                      hintText: context.t('onboarding.server.placeholder'),
                      hintStyle: ts(13, color: t.ink.withValues(alpha: .5), mono: true),
                      filled: true,
                      fillColor: t.pane,
                      contentPadding: const EdgeInsets.symmetric(horizontal: 12, vertical: 11),
                      border: OutlineInputBorder(borderRadius: t.inputRadius(40), borderSide: BorderSide(color: t.line)),
                      enabledBorder: OutlineInputBorder(borderRadius: t.inputRadius(40), borderSide: BorderSide(color: t.line)),
                      focusedBorder: OutlineInputBorder(borderRadius: t.inputRadius(40), borderSide: BorderSide(color: t.accent)),
                    ),
                  ),
                ),
              ),
              const SizedBox(width: 8),
              Container(
                height: 40,
                decoration: BoxDecoration(color: t.pane, borderRadius: t.inputRadius(40), border: Border.all(color: t.line)),
                child: Tap(
                  onTap: _url.text.trim().isEmpty || _testing ? null : _test,
                  radius: t.inputRadius(40),
                  padding: const EdgeInsets.symmetric(horizontal: 14),
                  child: Center(
                    child: Text(context.t(_testing ? 'onboarding.server.testing' : 'onboarding.server.test'),
                        style: ts(13, weight: 600, color: t.ink)),
                  ),
                ),
              ),
            ]),
            if (_tested != null) ...[
              _gap(8),
              Row(children: [
                AppIcon('okcircle', size: 15, color: t.accentText),
                const SizedBox(width: 6),
                Expanded(
                  child: Text(
                    '${_tested!.name} ${_tested!.version} · ${context.t(_tested!.registrationEnabled ? 'onboarding.server.registrationOpen' : 'onboarding.server.registrationClosed')}',
                    style: ts(12.5, color: t.accentText),
                  ),
                ),
              ]),
            ],
            if (_testError != null) ...[_gap(8), Text(_testError!, style: ts(12.5, color: t.danger))],
            _gap(8),
            Align(
              alignment: Alignment.centerLeft,
              child: LinkBtn(label: context.t('onboarding.server.reset'), color: t.muted, fontSize: 12.5, weight: 400, onPressed: _reset),
            ),
          ]),
        ),
      ],
    ]);
  }
}

// ---------------------------------------------------------------------------
// register -> recovery phrase
// ---------------------------------------------------------------------------

class _RegisterFlow extends StatefulWidget {
  const _RegisterFlow({required this.serverUrl, required this.onBack, this.resumeAccount, this.resumeMnemonic, required this.onStage});

  final String serverUrl;
  final VoidCallback onBack;
  final AccountInfo? resumeAccount;
  final List<String>? resumeMnemonic;

  /// Reports the step shown in the indicator: 0 = password, 1 = recovery phrase.
  final ValueChanged<int> onStage;

  @override
  State<_RegisterFlow> createState() => _RegisterFlowState();
}

class _RegisterFlowState extends State<_RegisterFlow> {
  final _pair = PasswordPair();
  RegisterResult? _result;
  bool _busy = false;
  String? _error;
  bool _savedIt = false;
  bool _copied = false;

  @override
  void initState() {
    super.initState();
    if (widget.resumeAccount != null && widget.resumeMnemonic != null) {
      _result = RegisterResult(account: widget.resumeAccount!, mnemonic: widget.resumeMnemonic!);
    }
  }

  @override
  void dispose() {
    _pair.dispose();
    super.dispose();
  }

  Future<void> _register() async {
    if (!_pair.valid) return;
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      final res = await backend.register(serverUrl: widget.serverUrl, password: _pair.password.text, deviceName: defaultDeviceName());
      _pair.reset();
      setState(() => _result = res);
      widget.onStage(1);
    } catch (e) {
      setState(() => _error = errorText(e));
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _confirm() async {
    if (!_savedIt) return;
    setState(() {
      _error = null;
      _busy = true;
    });
    try {
      await backend.confirmMnemonicSaved();
      // Next: name, username and appearance (setup_screen.dart).
      final id = _result?.account.accountId;
      if (id != null) prefsNotifier.update((p) => p.copyWith(setupPending: {...p.setupPending, id}));
      await enterApp();
    } catch (e) {
      if (mounted) {
        setState(() {
          _error = errorText(e);
          _busy = false;
        });
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    final r = _result;
    if (r == null) {
      return Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
        BackLink(onPressed: widget.onBack),
        _gap(),
        _Heading(context.t('onboarding.register.title'), context.t('onboarding.register.subtitle')),
        _gap(),
        PasswordPairFields(pair: _pair, label: context.t('password.label'), autofocus: true, large: true, onChanged: () => setState(() {})),
        _gap(),
        NoticeBox(text: context.t('onboarding.register.passwordHint')),
        if (_error != null) ...[_gap(12), _errorText(context, _error)],
        _gap(),
        Btn(
          label: context.t(_busy ? 'onboarding.register.creating' : 'onboarding.register.create'),
          busy: _busy,
          onPressed: _pair.valid ? _register : null,
        ),
      ]);
    }
    return Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
      _Heading(context.t('onboarding.seed.ready'), context.t('onboarding.seed.numberHint')),
      _gap(),
      Container(
        padding: const EdgeInsets.fromLTRB(18, 18, 18, 14),
        decoration: BoxDecoration(color: t.pane, borderRadius: BorderRadius.circular(t.rCard), border: Border.all(color: t.line)),
        child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
          Eyebrow(context.t('onboarding.seed.yourNumber')),
          _gap(10),
          Wrap(
            spacing: 12,
            runSpacing: 10,
            crossAxisAlignment: WrapCrossAlignment.center,
            alignment: WrapAlignment.spaceBetween,
            children: [
              Text(formatAccountNumber(r.account.number), style: ts(22, weight: 600, color: t.ink, mono: true, letterSpacing: .9)),
              ChipBtn(
                label: context.t(_copied ? 'common.copied' : 'common.copy'),
                icon: _copied ? 'check' : 'copy',
                onPressed: () async {
                  await Clipboard.setData(ClipboardData(text: r.account.number));
                  setState(() => _copied = true);
                  Timer(const Duration(milliseconds: 1600), () => mounted ? setState(() => _copied = false) : null);
                },
              ),
            ],
          ),
        ]),
      ),
      _gap(),
      Text(context.t('onboarding.seed.title'), style: ts(17, weight: 700, color: t.ink)),
      _gap(4),
      Text(context.t('onboarding.seed.subtitle'), style: ts(13.5, color: t.muted, height: 1.5)),
      _gap(),
      SeedGrid(words: r.mnemonic),
      _gap(),
      Semantics(
        checked: _savedIt,
        child: GestureDetector(
          behavior: HitTestBehavior.opaque,
          onTap: () => setState(() => _savedIt = !_savedIt),
          child: Padding(
            padding: const EdgeInsets.symmetric(vertical: 4),
            child: Row(children: [
              CheckBox(on: _savedIt),
              const SizedBox(width: 12),
              Expanded(child: Text(context.t('onboarding.seed.savedIt'), style: ts(14, color: t.ink))),
            ]),
          ),
        ),
      ),
      if (_error != null) ...[_gap(12), _errorText(context, _error)],
      _gap(),
      Btn(label: context.t('onboarding.seed.enter'), busy: _busy, onPressed: _savedIt ? _confirm : null),
    ]);
  }
}

// ---------------------------------------------------------------------------
// log in
// ---------------------------------------------------------------------------

class _LoginForm extends StatefulWidget {
  const _LoginForm({required this.serverUrl, required this.onBack, required this.onRecover});

  final String serverUrl;
  final VoidCallback onBack;
  final VoidCallback onRecover;

  @override
  State<_LoginForm> createState() => _LoginFormState();
}

class _LoginFormState extends State<_LoginForm> {
  final _number = TextEditingController();
  final _password = TextEditingController();
  final _code = TextEditingController();
  bool _busy = false;
  String? _error;
  // Two-step verification: asked after the server accepted the password.
  bool _needCode = false;
  // The account requires approval of new logins: waiting for one of its devices.
  bool _awaitingApproval = false;
  bool _approvalCancelled = false;

  bool get _valid =>
      digitsOnly(_number.text).length == 16 && _password.text.isNotEmpty && (!_needCode || digitsOnly(_code.text).length == 6);

  @override
  void dispose() {
    _number.dispose();
    _password.dispose();
    _code.dispose();
    super.dispose();
  }

  Future<void> _submit() async {
    if (!_valid) return;
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await backend.login(
        serverUrl: widget.serverUrl,
        number: digitsOnly(_number.text),
        password: _password.text,
        deviceName: defaultDeviceName(),
        totp: _needCode ? digitsOnly(_code.text) : null,
      );
      await enterApp();
    } catch (e) {
      if (mounted && parseError(e).code == 'approval_required') {
        await _waitForApproval();
        return;
      }
      if (mounted) {
        final code = parseError(e).code;
        setState(() {
          if (code == 'totp_required') {
            _needCode = true;
          } else {
            _error = errorText(e);
            if (code == 'invalid_totp') _code.clear();
          }
          _busy = false;
        });
      }
    }
  }

  Future<void> _waitForApproval() async {
    setState(() {
      _awaitingApproval = true;
      _approvalCancelled = false;
    });
    try {
      await backend.loginWaitApproval();
      await enterApp();
    } catch (e) {
      if (!mounted || _approvalCancelled) return;
      setState(() {
        _error = parseError(e).code == 'gone' ? context.t('security.loginExpired') : errorText(e);
        _awaitingApproval = false;
        _busy = false;
      });
    }
  }

  void _cancelApproval() {
    _approvalCancelled = true;
    unawaited(backend.loginCancelApproval());
    setState(() {
      _awaitingApproval = false;
      _busy = false;
      _error = null;
    });
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    if (_awaitingApproval) {
      return Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
        _Heading(context.t('security.waitingTitle'), context.t('security.waitingBody')),
        _gap(),
        Btn(label: context.t('common.cancel'), kind: BtnKind.secondary, onPressed: _cancelApproval),
      ]);
    }
    if (_needCode) {
      return Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
        BackLink(onPressed: () => setState(() {
          _needCode = false;
          _code.clear();
          _error = null;
        })),
        _gap(),
        _Heading(context.t('totp.loginTitle'), context.t('totp.loginHint')),
        _gap(),
        AppField(
          controller: _code,
          label: context.t('totp.codeLabel'),
          large: true,
          mono: true,
          autofocus: true,
          maxLength: 6,
          keyboardType: TextInputType.number,
          onChanged: (_) => setState(() {}),
          onSubmitted: (_) => _submit(),
        ),
        if (_error != null) ...[_gap(12), _errorText(context, _error)],
        _gap(),
        Btn(label: context.t(_busy ? 'onboarding.login.submitting' : 'totp.verify'), busy: _busy, onPressed: _valid ? _submit : null),
      ]);
    }
    return Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
      BackLink(onPressed: widget.onBack),
      _gap(),
      _Heading(context.t('onboarding.login.title'), context.t('onboarding.login.subtitle')),
      _gap(),
      AccountNumberField(controller: _number, label: context.t('onboarding.accountNumber'), autofocus: true, onChanged: (_) => setState(() {})),
      _gap(),
      AppField(
        controller: _password,
        label: context.t('password.label'),
        password: true,
        large: true,
        onChanged: (_) => setState(() {}),
        onSubmitted: (_) => _submit(),
      ),
      if (_error != null) ...[_gap(12), _errorText(context, _error)],
      _gap(),
      Btn(label: context.t(_busy ? 'onboarding.login.submitting' : 'onboarding.login.submit'), busy: _busy, onPressed: _valid ? _submit : null),
      _gap(),
      Row(children: [
        Expanded(child: Divider(color: t.line)),
        Padding(padding: const EdgeInsets.symmetric(horizontal: 12), child: Text(context.t('common.or'), style: ts(12.5, color: t.faint))),
        Expanded(child: Divider(color: t.line)),
      ]),
      _gap(),
      Btn(
        label: context.t('onboarding.login.scan'),
        icon: 'scan',
        kind: BtnKind.secondary,
        height: 48,
        fontSize: 14.5,
        onPressed: () => showLinkDevice(context, newDevice: true, serverUrl: widget.serverUrl),
      ),
      _gap(),
      LinkBtn(label: context.t('onboarding.login.recover'), onPressed: widget.onRecover),
    ]);
  }
}

// ---------------------------------------------------------------------------
// recover with the 24 words
// ---------------------------------------------------------------------------

const _wordCount = 24;

List<String> splitWords(String s) =>
    s.toLowerCase().replaceAll(RegExp(r'\d+[.)]'), ' ').split(RegExp(r'[\s,;]+')).where((w) => w.isNotEmpty).toList();

class _RecoverForm extends StatefulWidget {
  const _RecoverForm({required this.serverUrl, required this.onBack});

  final String serverUrl;
  final VoidCallback onBack;

  @override
  State<_RecoverForm> createState() => _RecoverFormState();
}

class _RecoverFormState extends State<_RecoverForm> {
  final _number = TextEditingController();
  final _words = List.generate(_wordCount, (_) => TextEditingController());
  final _focus = List.generate(_wordCount, (_) => FocusNode());
  final _pair = PasswordPair();
  bool _busy = false;
  String? _error;

  int get _filled => _words.where((w) => w.text.trim().isNotEmpty).length;
  bool get _valid => digitsOnly(_number.text).length == 16 && _filled == _wordCount && _pair.valid;

  @override
  void initState() {
    super.initState();
    for (final f in _focus) {
      f.addListener(() => setState(() {}));
    }
  }

  @override
  void dispose() {
    _number.dispose();
    for (final c in _words) {
      c.dispose();
    }
    for (final f in _focus) {
      f.dispose();
    }
    _pair.dispose();
    super.dispose();
  }

  void _onWord(int i, String raw) {
    final parts = splitWords(raw);
    final done = RegExp(r'[\s,;]$').hasMatch(raw);
    if (parts.length > 1 || (done && parts.length == 1)) {
      // Pasting several words fills the following fields; a space moves to the next field.
      for (var k = 0; k < parts.length && i + k < _wordCount; k++) {
        _words[i + k].text = parts[k];
      }
      // An unfinished last word keeps the focus so that typing continues in it.
      final next = (i + parts.length - (done ? 0 : 1)).clamp(0, _wordCount - 1);
      final c = _words[next];
      c.selection = TextSelection.collapsed(offset: c.text.length);
      _focus[next].requestFocus();
    }
    setState(() {});
  }

  Future<void> _submit() async {
    if (!_valid) return;
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await backend.recover(
        serverUrl: widget.serverUrl,
        number: digitsOnly(_number.text),
        mnemonic: _words.map((w) => w.text.trim().toLowerCase()).toList(),
        newPassword: _pair.password.text,
        deviceName: defaultDeviceName(),
      );
      await enterApp();
    } catch (e) {
      if (mounted) {
        setState(() {
          _error = errorText(e);
          _busy = false;
        });
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    return Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
      BackLink(onPressed: widget.onBack),
      _gap(),
      _Heading(context.t('onboarding.recover.title'), context.t('onboarding.recover.subtitle')),
      _gap(),
      AccountNumberField(controller: _number, label: context.t('onboarding.accountNumber'), autofocus: true, onChanged: (_) => setState(() {})),
      _gap(),
      Row(children: [
        Expanded(child: FieldLabel(context.t('onboarding.recover.words'))),
        Text('$_filled/$_wordCount', style: ts(12, color: _filled == _wordCount ? t.accentText : t.muted, mono: true)),
      ]),
      _gap(4),
      Text(context.t('onboarding.recover.pasteHint'), style: ts(12.5, color: t.muted)),
      _gap(8),
      LayoutBuilder(builder: (context, c) {
        final w = (c.maxWidth - 12) / 3;
        return Wrap(spacing: 6, runSpacing: 6, children: [
          for (var i = 0; i < _wordCount; i++)
            Container(
              width: w,
              height: 38,
              padding: const EdgeInsets.only(left: 10, right: 6),
              decoration: BoxDecoration(
                color: t.pane,
                borderRadius: BorderRadius.circular(10),
                border: Border.all(color: _focus[i].hasFocus ? t.accent : t.line),
              ),
              child: Row(children: [
                SizedBox(width: 16, child: Text('${i + 1}', textAlign: TextAlign.right, style: ts(11, color: t.faint, mono: true))),
                const SizedBox(width: 6),
                Expanded(
                  child: Semantics(
                    label: context.t('onboarding.seed.wordN', args: {'n': i + 1}),
                    child: TextField(
                      enableIMEPersonalizedLearning: imePersonalizedLearning,
                      controller: _words[i],
                      focusNode: _focus[i],
                      autocorrect: false,
                      enableSuggestions: false,
                      textInputAction: i == _wordCount - 1 ? TextInputAction.done : TextInputAction.next,
                      onChanged: (v) => _onWord(i, v),
                      onSubmitted: (_) => i < _wordCount - 1 ? _focus[i + 1].requestFocus() : null,
                      style: ts(13.5, color: t.ink, mono: true),
                      decoration: const InputDecoration(isDense: true, border: InputBorder.none, contentPadding: EdgeInsets.zero),
                    ),
                  ),
                ),
              ]),
            ),
        ]);
      }),
      _gap(),
      PasswordPairFields(pair: _pair, large: true, onChanged: () => setState(() {})),
      if (_error != null) ...[_gap(12), _errorText(context, _error)],
      _gap(),
      Btn(label: context.t('onboarding.recover.submit'), icon: 'key', busy: _busy, onPressed: _valid ? _submit : null),
    ]);
  }
}
