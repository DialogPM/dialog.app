// Settings menu (tab of the phone layout): profile row, sections, switch account, log out.

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../app/globals.dart';
import '../../app/nav.dart';
import '../../core/backend.dart' show parseError;
import '../../core/models.dart';
import '../../l10n/i18n.dart';
import '../../state/profiles.dart';
import '../../state/store.dart';
import '../../theme/app_theme.dart';
import '../../widgets/app_icon.dart';
import '../../state/actions.dart' show errorText;
import '../../widgets/dialogs.dart';
import '../../widgets/toast.dart';
import '../../widgets/ui.dart';
import 'logout.dart';

const kSettingsSections = <(String key, String icon)>[
  ('account', 'key'),
  ('privacy', 'lock'),
  ('notifications', 'bell'),
  ('devices', 'devices'),
  ('chats', 'chat'),
  ('appearance', 'palette'),
  ('language', 'lang'),
  ('about', 'info'),
];

/// Number of linked devices (for the "N active sessions" hint).
final deviceCountProvider = FutureProvider<int?>((ref) async {
  ref.watch(storeProvider.select((s) => s.devicesVersion));
  ref.watch(storeProvider.select((s) => s.appState?.account?.accountId));
  try {
    return (await backend.listDevices()).length;
  } catch (_) {
    return null;
  }
});

class SettingsMenu extends ConsumerWidget {
  const SettingsMenu({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final t = context.tk;
    final profile = ref.watch(storeProvider.select((s) => s.profile));
    final devices = ref.watch(deviceCountProvider).value;
    String hint(String key) {
      if (key == 'devices' && devices != null) return context.t('settings.devices.sessions', count: devices);
      if (key == 'language') return kLanguages.firstWhere((l) => l.code == context.lang, orElse: () => kLanguages.first).label;
      if (key == 'about') return context.t('settings.about.version', args: {'version': kAppVersion});
      return context.t('settings.$key.hint');
    }

    Widget row({required Widget leading, required Widget body, VoidCallback? onTap, Widget? trailing, double vertical = 11}) => Padding(
          padding: EdgeInsets.symmetric(horizontal: t.rowMx),
          child: Tap(
            onTap: onTap,
            radius: BorderRadius.circular(t.rowRadius),
            padding: EdgeInsets.symmetric(horizontal: t.rowPx, vertical: vertical),
            child: Row(children: [leading, const SizedBox(width: 14), Expanded(child: body), ?trailing]),
          ),
        );

    return Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
      Padding(
        padding: const EdgeInsets.fromLTRB(20, 18, 20, 12),
        child: Text(context.t('settings.title'), style: ts(25, weight: 750, color: t.ink, letterSpacing: -.6)),
      ),
      Expanded(
        child: ListView(padding: const EdgeInsets.only(bottom: 12), children: [
          row(
            vertical: 14,
            onTap: () => openSettingsSection('profile'),
            leading: Avatar(name: (profile?.name.isNotEmpty ?? false) ? profile!.name : '?', path: profile?.avatarPath, seed: 'me', size: 58),
            body: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
              Text(
                (profile?.name.isNotEmpty ?? false) ? profile!.name : context.t('settings.profile.noName'),
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: ts(17, weight: 700, color: t.ink),
              ),
              Text(
                (profile?.about.isNotEmpty ?? false) ? profile!.about : context.t('settings.profile.aboutPlaceholder'),
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: ts(13.5, color: t.muted),
              ),
            ]),
            trailing: AppIcon('chevron', size: 18, color: t.faint),
          ),
          const SizedBox(height: 8),
          for (final (key, icon) in kSettingsSections)
            row(
              onTap: () => openSettingsSection(key),
              leading: Container(
                width: 38,
                height: 38,
                decoration: BoxDecoration(color: t.sunk, borderRadius: BorderRadius.circular(11)),
                child: Center(child: AppIcon(icon, size: 19, color: t.accentText)),
              ),
              body: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
                Text(context.t('settings.$key.title'), style: ts(15, weight: 600, color: t.ink)),
                Text(hint(key), maxLines: 1, overflow: TextOverflow.ellipsis, style: ts(12.5, color: t.muted)),
              ]),
            ),
          const SizedBox(height: 8),
          const _QuickAccounts(),
          // Keeps this account on the phone (locked, still logged in) and opens the start screen.
          row(
            onTap: () => switchAccount(),
            leading: Container(
              width: 38,
              height: 38,
              decoration: BoxDecoration(color: t.sunk, borderRadius: BorderRadius.circular(11)),
              child: Center(child: AppIcon('users', size: 19, color: t.accentText)),
            ),
            body: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
              Text(context.t('profiles.switch'), style: ts(15, weight: 600, color: t.ink)),
              Text(context.t('profiles.switchHint'), maxLines: 2, overflow: TextOverflow.ellipsis, style: ts(12.5, color: t.muted)),
            ]),
          ),
          row(
            onTap: () => confirmLogout(context),
            leading: SizedBox(width: 38, height: 38, child: Center(child: AppIcon('logout', size: 19, color: t.danger))),
            body: Text(context.t('mobile.profiles.remove'), style: ts(15, weight: 600, color: t.danger)),
          ),
        ]),
      ),
    ]);
  }
}

/// The other accounts on this phone, one tap away: an account opened with its password since
/// the app started switches at once, the others ask for the password first.
class _QuickAccounts extends StatefulWidget {
  const _QuickAccounts();

  @override
  State<_QuickAccounts> createState() => _QuickAccountsState();
}

class _QuickAccountsState extends State<_QuickAccounts> {
  List<ProfileCard> _others = const [];

  @override
  void initState() {
    super.initState();
    backend.listProfiles().then((list) {
      if (mounted) setState(() => _others = list.where((p) => !p.active).toList());
    }, onError: (_) {});
  }

  Future<void> _switch(ProfileCard p) async {
    if (await quickSwitch(p) || !mounted) return;
    final name = profileName(p, context.t('profiles.account'));
    final pw = TextEditingController();
    await showAppDialog<void>(context, title: context.t('profiles.passwordFor', args: {'name': name}), builder: (ctx) {
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
            if (ctx.mounted) Navigator.of(ctx).pop();
            await openProfile(p.id, pw.text);
          } catch (e) {
            showToast(parseError(e).code == 'invalid_password' ? tr('dataLock.wrongPassword') : errorText(e), error: true);
          }
        }

        return Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
          AppField(controller: pw, label: ctx.t('password.label'), password: true, error: error, autofocus: true, onSubmitted: (_) => submit(), onChanged: (_) => setState(() {})),
          const SizedBox(height: 14),
          Btn(label: ctx.t('profiles.open'), busy: busy, onPressed: pw.text.isEmpty ? null : submit),
        ]);
      });
    });
    pw.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    return Column(children: [
      for (final p in _others)
        Padding(
          padding: EdgeInsets.symmetric(horizontal: t.rowMx),
          child: Tap(
            onTap: () => _switch(p),
            radius: BorderRadius.circular(t.rowRadius),
            padding: EdgeInsets.symmetric(horizontal: t.rowPx, vertical: 9),
            child: Row(children: [
              Avatar(name: profileName(p, context.t('profiles.account')), seed: p.id, size: 38),
              const SizedBox(width: 14),
              Expanded(
                child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
                  Text(profileName(p, context.t('profiles.account')), maxLines: 1, overflow: TextOverflow.ellipsis, style: ts(15, weight: 600, color: t.ink)),
                  Text(context.t(p.unlocked ? 'profiles.unlockedHint' : 'profiles.lockedHint'), maxLines: 1, overflow: TextOverflow.ellipsis, style: ts(12.5, color: t.muted)),
                ]),
              ),
              if (!p.unlocked) AppIcon('lock', size: 15, color: t.faint),
            ]),
          ),
        ),
    ]);
  }
}
