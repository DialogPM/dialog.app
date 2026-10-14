// Start screen: the accounts saved on this phone (local profiles, state/profiles.dart). Each one
// opens with its own password, locally, without logging in again. Hidden accounts are opened by
// typing the full account number ("Open another account on this phone"). Port of the desktop
// app/src/features/onboarding/ProfilePicker.tsx.

import 'package:flutter/material.dart';

import '../../app/globals.dart';
import '../../core/backend.dart' show parseError;
import '../../core/models.dart';
import '../../l10n/i18n.dart';
import '../../state/actions.dart';
import '../../state/profiles.dart';
import '../../theme/app_theme.dart';
import '../../util/format.dart';
import '../../widgets/app_icon.dart';
import '../../widgets/dialogs.dart';
import '../../widgets/ui.dart';

/// The listed profiles; a tap shows the password field. [onChanged]: the list changed (a
/// profile's local data was deleted).
class ProfileList extends StatefulWidget {
  const ProfileList({super.key, required this.profiles, required this.onChanged});

  final List<ProfileCard> profiles;
  final VoidCallback onChanged;

  @override
  State<ProfileList> createState() => _ProfileListState();
}

class _ProfileListState extends State<ProfileList> {
  late String? _open = initiallyOpenProfile(widget.profiles);

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    return Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
      Text(context.t('mobile.profiles.title'), style: ts(27, weight: 700, color: t.ink, letterSpacing: -0.54)),
      const SizedBox(height: 6),
      Text(context.t('profiles.lead'), style: ts(14.5, color: t.muted, height: 1.5)),
      const SizedBox(height: 18),
      ListCard(children: [
        for (final p in widget.profiles)
          Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
            _ProfileRow(
              profile: p,
              open: _open == p.id,
              onTap: () async {
                // Opened earlier in this run of the app: no password needed.
                if (p.unlocked && await quickSwitch(p)) return;
                if (mounted) setState(() => _open = _open == p.id ? null : p.id);
              },
            ),
            if (_open == p.id) _UnlockForm(key: ValueKey(p.id), profile: p, onForgotten: widget.onChanged),
          ]),
      ]),
    ]);
  }
}

class _ProfileRow extends StatelessWidget {
  const _ProfileRow({required this.profile, required this.open, required this.onTap});

  final ProfileCard profile;
  final bool open;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    final name = profileName(profile, context.t('profiles.account'));
    final sub = profileSubtitle(profile);
    return Tap(
      onTap: onTap,
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
      child: Row(children: [
        Avatar(name: name, seed: profile.id, size: 42),
        const SizedBox(width: 12),
        Expanded(
          child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
            Text(name, maxLines: 1, overflow: TextOverflow.ellipsis, style: ts(15.5, weight: 650, color: t.ink)),
            if (sub.isNotEmpty) Text(sub, maxLines: 1, overflow: TextOverflow.ellipsis, style: ts(12.5, color: t.muted, mono: true)),
          ]),
        ),
        if (profile.unlocked)
          Text(context.t('profiles.unlocked'), style: ts(13, weight: 600, color: t.accentText))
        else
          AnimatedRotation(
            turns: open ? .25 : 0,
            duration: const Duration(milliseconds: 150),
            child: AppIcon('chevron', size: 17, color: t.faint),
          ),
      ]),
    );
  }
}

/// Password form of one profile.
class _UnlockForm extends StatefulWidget {
  const _UnlockForm({super.key, required this.profile, required this.onForgotten});

  final ProfileCard profile;
  final VoidCallback onForgotten;

  @override
  State<_UnlockForm> createState() => _UnlockFormState();
}

class _UnlockFormState extends State<_UnlockForm> {
  final _password = TextEditingController();
  bool _busy = false;
  String? _error;

  @override
  void dispose() {
    _password.dispose();
    super.dispose();
  }

  Future<void> _submit() async {
    if (_password.text.isEmpty || _busy) return;
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await openProfile(widget.profile.id, _password.text);
    } catch (e) {
      if (!mounted) return;
      setState(() {
        _error = parseError(e).code == 'invalid_password' ? context.t('dataLock.wrongPassword') : errorText(e);
        _busy = false;
      });
    }
  }

  Future<void> _forgot() async {
    final ok = await showConfirm(
      context,
      title: context.t('profiles.forgetTitle'),
      body: context.t('mobile.profiles.forgetBody'),
      confirmLabel: context.t('profiles.forget'),
    );
    if (!ok) return;
    final done = await attempt(() async {
      await backend.forgetProfile(widget.profile.id);
      return true;
    });
    if (done == true) widget.onForgotten();
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    final name = profileName(widget.profile, context.t('profiles.account'));
    return Padding(
      padding: const EdgeInsets.fromLTRB(16, 4, 16, 14),
      child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
        AppField(
          controller: _password,
          label: context.t('profiles.passwordFor', args: {'name': name}),
          password: true,
          autofocus: true,
          error: _error,
          onChanged: (_) => setState(() {}),
          onSubmitted: (_) => _submit(),
        ),
        const SizedBox(height: 12),
        Btn(
          label: context.t(_busy ? 'profiles.opening' : 'profiles.open'),
          busy: _busy,
          height: 46,
          onPressed: _password.text.isEmpty ? null : _submit,
        ),
        const SizedBox(height: 6),
        Center(child: LinkBtn(label: context.t('profiles.forgot'), color: t.muted, fontSize: 13, weight: 500, onPressed: _busy ? null : _forgot)),
      ]),
    );
  }
}

/// "Open another account on this phone": full account number and password (also finds the
/// accounts hidden from the start screen).
class OpenOtherProfile extends StatefulWidget {
  const OpenOtherProfile({super.key, required this.onBack});

  final VoidCallback onBack;

  @override
  State<OpenOtherProfile> createState() => _OpenOtherProfileState();
}

class _OpenOtherProfileState extends State<OpenOtherProfile> {
  final _number = TextEditingController();
  final _password = TextEditingController();
  bool _busy = false;
  String? _error;

  bool get _valid => digitsOnly(_number.text).length == 16 && _password.text.isNotEmpty;

  @override
  void dispose() {
    _number.dispose();
    _password.dispose();
    super.dispose();
  }

  Future<void> _submit() async {
    if (!_valid || _busy) return;
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await openProfileByNumber(digitsOnly(_number.text), _password.text);
    } catch (e) {
      if (!mounted) return;
      setState(() {
        _error = parseError(e).code == 'invalid_password' ? context.t('profiles.otherWrong') : errorText(e);
        _busy = false;
      });
    }
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    return Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
      BackLink(onPressed: widget.onBack),
      const SizedBox(height: 18),
      Text(context.t('profiles.otherTitle'), style: ts(27, weight: 700, color: t.ink, letterSpacing: -0.54)),
      const SizedBox(height: 6),
      Text(context.t('profiles.otherLead'), style: ts(14.5, color: t.muted, height: 1.5)),
      const SizedBox(height: 18),
      AccountNumberField(controller: _number, label: context.t('onboarding.accountNumber'), autofocus: true, onChanged: (_) => setState(() {})),
      const SizedBox(height: 18),
      AppField(
        controller: _password,
        label: context.t('password.label'),
        password: true,
        large: true,
        onChanged: (_) => setState(() {}),
        onSubmitted: (_) => _submit(),
      ),
      if (_error != null) ...[const SizedBox(height: 12), Text(_error!, style: ts(13, color: t.danger, height: 1.45))],
      const SizedBox(height: 18),
      Btn(label: context.t(_busy ? 'profiles.opening' : 'profiles.open'), busy: _busy, onPressed: _valid ? _submit : null),
    ]);
  }
}
