// Settings sections (one full-screen page each on phones).

import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_contacts/flutter_contacts.dart' as fc;
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:url_launcher/url_launcher.dart';

import '../../app/globals.dart';
import '../../app/nav.dart';
import '../../core/backend.dart';
import '../../core/models.dart';
import '../../l10n/i18n.dart';
import '../../services/battery.dart';
import '../../services/files.dart';
import '../../services/media.dart';
import '../../services/notifications.dart';
import '../../services/push.dart';
import '../../state/actions.dart';
import '../../state/call_settings.dart';
import '../../state/default_timer.dart';
import '../../state/dnd.dart';
import '../../state/media_retention.dart';
import '../../state/prefs.dart';
import '../../state/store.dart';
import '../../theme/app_theme.dart';
import '../../theme/tokens.dart';
import '../../util/format.dart';
import '../../widgets/app_icon.dart';
import '../../widgets/dialogs.dart';
import '../../widgets/ui.dart';
import '../chats/contact_picker.dart';
import '../chats/new_group_page.dart';
import '../devices/link_device.dart';
import 'username_link.dart';
import 'voice_mask_card.dart';

class SettingsSectionPage extends StatelessWidget {
  const SettingsSectionPage({super.key, required this.section});
  final String section;

  @override
  Widget build(BuildContext context) {
    final Widget body = switch (section) {
      'profile' => const _ProfileSection(),
      'account' => const _AccountSection(),
      'privacy' => const _PrivacySection(),
      'notifications' => const _NotificationsSection(),
      'devices' => const _DevicesSection(),
      'chats' => const _ChatsSection(),
      'appearance' => const AppearanceSection(),
      'language' => const _LanguageSection(),
      _ => const _AboutSection(),
    };
    return PaneScaffold(
      header: PaneHeader(title: context.t('settings.$section.title')),
      body: body,
    );
  }
}

/// Scrollable body of a section: padding 20/18, gap 16, max width 620.
class _Body extends StatelessWidget {
  const _Body({required this.children, this.embedded = false});
  final List<Widget> children;

  /// Inside another scrolling page (the first-run setup): no scroll view of its own.
  final bool embedded;

  @override
  Widget build(BuildContext context) {
    final items = <Widget>[];
    for (var i = 0; i < children.length; i++) {
      if (i > 0) items.add(const SizedBox(height: 16));
      items.add(children[i]);
    }
    if (embedded) return Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: items);
    return ListView(padding: const EdgeInsets.fromLTRB(18, 20, 18, 28), children: [
      Center(
        child: ConstrainedBox(
          constraints: const BoxConstraints(maxWidth: 620),
          child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: items),
        ),
      ),
    ]);
  }
}

class _Hint extends StatelessWidget {
  const _Hint(this.text, {this.size = 12.5});
  final String text;
  final double size;

  @override
  Widget build(BuildContext context) => Text(text, style: ts(size, color: context.tk.muted, height: 1.5));
}

class _SubLabel extends StatelessWidget {
  const _SubLabel(this.text);
  final String text;

  @override
  Widget build(BuildContext context) => Text(text, style: ts(13, weight: 600, color: context.tk.muted));
}

// ---------------------------------------------------------------------------
// profile
// ---------------------------------------------------------------------------

class _ProfileSection extends ConsumerStatefulWidget {
  const _ProfileSection();

  @override
  ConsumerState<_ProfileSection> createState() => _ProfileSectionState();
}

class _ProfileSectionState extends ConsumerState<_ProfileSection> {
  late final _name = TextEditingController(text: appData.profile?.name ?? '');
  late final _about = TextEditingController(text: appData.profile?.about ?? '');
  final _nameFocus = FocusNode();
  final _aboutFocus = FocusNode();

  @override
  void initState() {
    super.initState();
    _nameFocus.addListener(() => _nameFocus.hasFocus ? null : _save());
    _aboutFocus.addListener(() => _aboutFocus.hasFocus ? null : _save());
  }

  @override
  void dispose() {
    _save();
    _name.dispose();
    _about.dispose();
    _nameFocus.dispose();
    _aboutFocus.dispose();
    super.dispose();
  }

  // Saved when a field loses focus (or on Enter), like the design which has no save button.
  Future<void> _save() async {
    final p0 = appData.profile;
    final n = _name.text.trim();
    final a = _about.text.trim();
    if (n.isEmpty || (n == (p0?.name ?? '') && a == (p0?.about ?? ''))) {
      if (n.isEmpty) _name.text = p0?.name ?? '';
      return;
    }
    final p = await attempt(() => backend.setProfile(name: n, about: a));
    if (p != null) {
      store.set((d) => d.copyWith(profile: p));
      notifyOk(tr('common.saved'));
    }
  }

  Future<void> _photo() async {
    final profile = appData.profile;
    await showActionSheet(context, actions: [
      SheetAction(
        label: context.t('mobile.takePhoto'),
        icon: 'camera',
        hidden: !isMobilePlatform,
        onTap: () async {
          final f = await takePhoto();
          if (f == null) return;
          final data = base64Encode(await File(f.path).readAsBytes());
          unawaited(deleteTempCopy(f.path));
          final p = await attempt(() => backend.setProfile(avatarDataBase64: data));
          if (p != null) store.set((d) => d.copyWith(profile: p));
        },
      ),
      SheetAction(
        label: context.t('mobile.choosePhoto'),
        icon: 'image',
        onTap: () async {
          final path = await pickImagePath();
          if (path == null) return;
          final p = await attempt(() => backend.setProfile(avatarPath: path));
          unawaited(deleteTempCopy(path));
          if (p != null) store.set((d) => d.copyWith(profile: p));
        },
      ),
      SheetAction(
        label: context.t('profile.removePhoto'),
        icon: 'trash',
        danger: true,
        hidden: profile?.avatarPath == null,
        onTap: () async {
          final p = await attempt(() => backend.setProfile(avatarPath: ''));
          if (p != null) store.set((d) => d.copyWith(profile: p));
        },
      ),
    ]);
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    final profile = ref.watch(storeProvider.select((s) => s.profile));
    return _Body(children: [
      Center(
        child: Padding(
          padding: const EdgeInsets.only(top: 8, bottom: 10),
          child: Semantics(
            button: true,
            label: context.t('profile.changePhoto'),
            child: GestureDetector(
              onTap: _photo,
              child: Stack(clipBehavior: Clip.none, children: [
                Avatar(name: _name.text.isEmpty ? '?' : _name.text, path: profile?.avatarPath, seed: 'me', size: 124),
                Positioned(
                  right: -4,
                  bottom: -2,
                  child: Container(
                    width: 46,
                    height: 46,
                    decoration: BoxDecoration(color: t.accent, shape: BoxShape.circle, border: Border.all(color: t.pane, width: 4)),
                    child: Center(child: AppIcon('camera', size: 17, color: t.onAccent)),
                  ),
                ),
              ]),
            ),
          ),
        ),
      ),
      AppField(
        controller: _name,
        focusNode: _nameFocus,
        label: context.t('settings.profile.name'),
        mutedLabel: true,
        maxLength: 60,
        onChanged: (_) => setState(() {}),
        onSubmitted: (_) => _save(),
      ),
      AppField(
        controller: _about,
        focusNode: _aboutFocus,
        label: context.t('settings.profile.about'),
        hint: context.t('settings.profile.aboutPlaceholder'),
        mutedLabel: true,
        maxLength: 140,
        onSubmitted: (_) => _save(),
      ),
      _Hint(context.t('settings.profile.visibility')),
    ]);
  }
}

// ---------------------------------------------------------------------------
// account
// ---------------------------------------------------------------------------

final _usernameRe = RegExp(r'^[a-z][a-z0-9_.]{2,31}$');

class _AccountSection extends ConsumerWidget {
  const _AccountSection();

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final t = context.tk;
    final details = ref.watch(storeProvider.select((s) => s.details));
    if (details == null) return const Spinner();
    final number = details.account.number;
    final username = details.username;
    return _Body(children: [
      Container(
        padding: const EdgeInsets.all(18),
        decoration: BoxDecoration(color: t.sunk, borderRadius: BorderRadius.circular(t.rCard)),
        child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
          Eyebrow(context.t('settings.account.number')),
          const SizedBox(height: 8),
          Wrap(
            spacing: 12,
            runSpacing: 8,
            crossAxisAlignment: WrapCrossAlignment.center,
            alignment: WrapAlignment.spaceBetween,
            children: [
              Text(formatAccountNumber(number), style: ts(20, weight: 600, color: t.ink, mono: true, letterSpacing: .6)),
              ChipBtn(
                label: context.t('common.copy'),
                icon: 'copy',
                tone: 'pane',
                onPressed: () async {
                  await Clipboard.setData(ClipboardData(text: number));
                  notifyOk(tr('common.copied'));
                },
              ),
            ],
          ),
          const SizedBox(height: 8),
          Text.rich(
            TextSpan(children: [
              TextSpan(text: '${context.t('settings.account.server')}: '),
              TextSpan(text: details.account.serverUrl, style: ts(12.5, color: t.muted, mono: true)),
            ]),
            style: ts(12.5, color: t.muted),
          ),
        ]),
      ),
      ListCard(children: [
        ListRow(
          icon: 'user',
          title: context.t('mobile.username'),
          sub: username != null ? '@$username' : context.t('mobile.usernameNone'),
          onTap: () => _editUsername(context, username),
        ),
        if (username != null)
          ListRow(icon: 'qr', title: context.t('settings.username.linkTitle'), sub: context.t('settings.username.linkShow'), onTap: () => showUsernameLink(context)),
        ListRow(icon: 'key', title: context.t('settings.account.changePassword'), sub: context.t('settings.account.changePasswordHint'), onTap: () => _changePassword(context)),
        ListRow(icon: 'eye', title: context.t('settings.account.recovery'), sub: context.t('settings.account.recoveryHint'), onTap: () => _revealMnemonic(context)),
        ListRow(icon: 'qr', title: context.t('qr.myCode'), sub: context.t('settings.account.qrHint'), onTap: () => openQr()),
      ]),
      const _PhoneCard(),
      const _ThisPhoneCard(),
      Align(
        alignment: Alignment.centerLeft,
        child: GestureDetector(
          onTap: () => _deleteAccount(context),
          child: Padding(
            padding: const EdgeInsets.symmetric(vertical: 6),
            child: Row(mainAxisSize: MainAxisSize.min, children: [
              AppIcon('trash', size: 17, color: t.danger),
              const SizedBox(width: 8),
              Text(context.t('settings.account.delete'), style: ts(14.5, weight: 650, color: t.danger)),
            ]),
          ),
        ),
      ),
    ]);
  }
}

Future<void> _editUsername(BuildContext context, String? current) async {
  final c = TextEditingController(text: current ?? '');
  await showAppDialog<void>(context, title: context.t('mobile.username'), builder: (ctx) {
    String? error;
    var busy = false;
    return StatefulBuilder(builder: (ctx, setState) {
      final t = ctx.tk;
      final v = c.text.trim().replaceFirst(RegExp('^@'), '').toLowerCase();
      final valid = _usernameRe.hasMatch(v) && usernameDiscriminatorValid(v);
      Future<void> save(String? value) async {
        setState(() {
          busy = true;
          error = null;
        });
        try {
          final res = await backend.setUsername(value);
          store.set((d) => d.copyWith(details: d.details?.copyWith(username: res, clearUsername: res == null)));
          if (ctx.mounted) Navigator.of(ctx).pop();
          notifyOk(tr('mobile.usernameSaved'));
        } catch (e) {
          final code = parseError(e).code;
          setState(() {
            busy = false;
            error = code == 'conflict' ? tr('mobile.usernameTaken') : code == 'invalid_input' ? tr('mobile.usernameInvalid') : errorText(e);
          });
        }
      }

      return Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
        Text(ctx.t('mobile.usernameHint'), style: ts(13, color: t.muted, height: 1.45)),
        const SizedBox(height: 14),
        AppField(
          controller: c,
          autofocus: true,
          hint: 'name',
          maxLength: 33,
          suffix: null,
          error: error ??
              (v.isNotEmpty && !valid
                  ? ctx.t(_usernameRe.hasMatch(v) ? 'settings.username.errNumber' : 'mobile.usernameInvalid')
                  : null),
          onChanged: (_) => setState(() {}),
          inputFormatters: [FilteringTextInputFormatter.allow(RegExp(r'[@A-Za-z0-9_.]'))],
        ),
        const SizedBox(height: 8),
        Text(ctx.t('settings.username.numberHint'), style: ts(12.5, color: t.muted, height: 1.4)),
        Align(
          alignment: Alignment.centerLeft,
          child: LinkBtn(
            label: ctx.t('settings.username.addNumber'),
            fontSize: 13,
            onPressed: RegExp('^[a-z]').hasMatch(v)
                ? () => setState(() {
                      final next = withRandomDiscriminator(v);
                      c.value = TextEditingValue(text: next, selection: TextSelection.collapsed(offset: next.length));
                      error = null;
                    })
                : null,
          ),
        ),
        const SizedBox(height: 12),
        Row(children: [
          if (current != null) LinkBtn(label: ctx.t('mobile.usernameRemove'), color: t.danger, fontSize: 13, onPressed: busy ? null : () => save(null)),
          const Spacer(),
          Btn(label: ctx.t('common.cancel'), kind: BtnKind.secondary, height: 42, expand: false, onPressed: () => Navigator.of(ctx).pop()),
          const SizedBox(width: 8),
          Btn(label: ctx.t('common.save'), height: 42, expand: false, busy: busy, onPressed: valid && v != current ? () => save(v) : null),
        ]),
      ]);
    });
  });
}

Future<void> _changePassword(BuildContext context) async {
  final old = TextEditingController();
  final pair = PasswordPair();
  await showAppDialog<void>(context, title: context.t('settings.account.changePassword'), builder: (ctx) {
    String? error;
    var busy = false;
    return StatefulBuilder(builder: (ctx, setState) {
      Future<void> submit() async {
        setState(() {
          busy = true;
          error = null;
        });
        try {
          await backend.changePassword(oldPassword: old.text, newPassword: pair.password.text);
          if (ctx.mounted) Navigator.of(ctx).pop();
          notifyOk(tr('settings.account.passwordChanged'));
        } catch (e) {
          setState(() {
            error = errorText(e);
            busy = false;
          });
        }
      }

      return Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
        AppField(controller: old, label: ctx.t('password.current'), password: true, autofocus: true, onChanged: (_) => setState(() {})),
        const SizedBox(height: 14),
        PasswordPairFields(pair: pair, onChanged: () => setState(() {})),
        if (error != null) ...[const SizedBox(height: 12), Text(error!, style: ts(13, color: ctx.tk.danger))],
        const SizedBox(height: 18),
        DialogActions(children: [
          Btn(label: ctx.t('common.cancel'), kind: BtnKind.secondary, height: 42, expand: false, onPressed: () => Navigator.of(ctx).pop()),
          Btn(
            label: ctx.t('settings.account.changePassword'),
            height: 42,
            expand: false,
            busy: busy,
            onPressed: old.text.isNotEmpty && pair.valid ? submit : null,
          ),
        ]),
      ]);
    });
  });
}

/// Asks for the password, then shows the 24 words (blurred until revealed).
Future<void> _revealMnemonic(BuildContext context) async {
  final pw = TextEditingController();
  await showAppDialog<void>(context, title: context.t('settings.account.recovery'), builder: (ctx) {
    List<String>? words;
    String? error;
    var busy = false;
    return StatefulBuilder(builder: (ctx, setState) {
      final t = ctx.tk;
      if (words != null) {
        return Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
          Text(ctx.t('settings.account.recoveryShown'), style: ts(13.5, color: t.muted, height: 1.45)),
          const SizedBox(height: 16),
          SeedGrid(words: words!),
          const SizedBox(height: 16),
          NoticeBox(tone: 'warn', icon: 'alert', text: ctx.t('onboarding.seed.warning')),
          const SizedBox(height: 16),
          DialogActions(children: [Btn(label: ctx.t('common.close'), height: 42, expand: false, onPressed: () => Navigator.of(ctx).pop())]),
        ]);
      }
      Future<void> submit() async {
        setState(() {
          busy = true;
          error = null;
        });
        try {
          final w = await backend.revealMnemonic(pw.text);
          pw.clear();
          setState(() => words = w);
        } catch (e) {
          setState(() => error = parseError(e).code == 'invalid_password' ? tr('lock.wrongPassword') : errorText(e));
        } finally {
          setState(() => busy = false);
        }
      }

      return Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
        Text(ctx.t('settings.account.recoveryAsk'), style: ts(13.5, color: t.muted, height: 1.45)),
        const SizedBox(height: 14),
        AppField(
          controller: pw,
          label: ctx.t('password.label'),
          password: true,
          autofocus: true,
          error: error,
          onChanged: (_) => setState(() {}),
          onSubmitted: (_) => pw.text.isEmpty ? null : submit(),
        ),
        const SizedBox(height: 18),
        DialogActions(children: [
          Btn(label: ctx.t('common.cancel'), kind: BtnKind.secondary, height: 42, expand: false, onPressed: () => Navigator.of(ctx).pop()),
          Btn(label: ctx.t('settings.account.recoveryShow'), icon: 'eye', height: 42, expand: false, busy: busy, onPressed: pw.text.isEmpty ? null : submit),
        ]),
      ]);
    });
  });
}

Future<void> _deleteAccount(BuildContext context) async {
  final pw = TextEditingController();
  final confirm = TextEditingController();
  final last4 = (appData.details?.account.number ?? '').padLeft(4).substring((appData.details?.account.number.length ?? 4) - 4);
  await showAppDialog<void>(context, title: context.t('settings.account.delete'), builder: (ctx) {
    String? error;
    var busy = false;
    return StatefulBuilder(builder: (ctx, setState) {
      Future<void> submit() async {
        setState(() {
          busy = true;
          error = null;
        });
        try {
          await backend.deleteAccount(pw.text);
          if (ctx.mounted) Navigator.of(ctx).pop();
          signedOut('account_deleted');
        } catch (e) {
          setState(() {
            error = errorText(e);
            busy = false;
          });
        }
      }

      return Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
        NoticeBox(tone: 'danger', icon: 'alert', text: ctx.t('settings.account.deleteWarning')),
        const SizedBox(height: 14),
        AppField(controller: pw, label: ctx.t('password.label'), password: true, onChanged: (_) => setState(() {})),
        const SizedBox(height: 14),
        AppField(
          controller: confirm,
          label: ctx.t('settings.account.deleteConfirm', args: {'digits': last4}),
          mono: true,
          keyboardType: TextInputType.number,
          onChanged: (_) => setState(() {}),
        ),
        if (error != null) ...[const SizedBox(height: 12), Text(error!, style: ts(13, color: ctx.tk.danger))],
        const SizedBox(height: 18),
        DialogActions(children: [
          Btn(label: ctx.t('common.cancel'), kind: BtnKind.secondary, height: 42, expand: false, onPressed: () => Navigator.of(ctx).pop()),
          Btn(
            label: ctx.t('settings.account.deleteForever'),
            kind: BtnKind.danger,
            height: 42,
            expand: false,
            busy: busy,
            onPressed: pw.text.isNotEmpty && confirm.text.trim() == last4 ? submit : null,
          ),
        ]),
      ]);
    });
  });
}

/// "This phone": whether this account is listed on the start screen (accounts saved on this
/// phone, state/profiles.dart). Shown once the account is saved here.
class _ThisPhoneCard extends StatefulWidget {
  const _ThisPhoneCard();

  @override
  State<_ThisPhoneCard> createState() => _ThisPhoneCardState();
}

class _ThisPhoneCardState extends State<_ThisPhoneCard> {
  CurrentProfile? _current;

  @override
  void initState() {
    super.initState();
    backend.currentProfile().then((c) {
      if (mounted) setState(() => _current = c);
    }).catchError((Object e) => debugPrint('profile: $e'));
  }

  Future<void> _toggle() async {
    final c = _current;
    if (c == null) return;
    final visible = !c.visible;
    final ok = await attempt(() async {
      await backend.setProfileVisible(visible);
      return true;
    });
    if (ok == true && mounted) setState(() => _current = CurrentProfile(registered: true, visible: visible));
  }

  @override
  Widget build(BuildContext context) {
    final c = _current;
    if (c == null || !c.registered) return const SizedBox.shrink();
    return Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
      _SubLabel(context.t('mobile.profiles.thisPhone')),
      const SizedBox(height: 8),
      ListCard(children: [
        ToggleRow(
          padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 15),
          label: context.t('profiles.showOnStart'),
          sub: context.t('mobile.profiles.showOnStartHint'),
          on: c.visible,
          onToggle: _toggle,
        ),
      ]),
    ]);
  }
}

class _PhoneCard extends ConsumerStatefulWidget {
  const _PhoneCard();

  @override
  ConsumerState<_PhoneCard> createState() => _PhoneCardState();
}

class _PhoneCardState extends ConsumerState<_PhoneCard> {
  final _phone = TextEditingController();
  final _code = TextEditingController();
  bool _sent = false;
  bool _busy = false;
  String? _error;

  @override
  void dispose() {
    _phone.dispose();
    _code.dispose();
    super.dispose();
  }

  Future<void> _run(Future<void> Function() fn) async {
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await fn();
    } catch (e) {
      if (mounted) setState(() => _error = errorText(e));
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    final details = ref.watch(storeProvider.select((s) => s.details))!;
    final server = ref.watch(storeProvider.select((s) => s.serverInfo));
    final supported = server?.phoneVerificationEnabled ?? true;
    Widget body;
    if (!supported) {
      body = _Hint(context.t('settings.account.phoneUnsupported'));
    } else if (details.phone != null && details.phoneVerified) {
      body = Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
        Row(children: [
          Flexible(child: Text(details.phone!, style: ts(15, weight: 600, color: t.ink))),
          const SizedBox(width: 10),
          SoftBadge(context.t('settings.account.verified')),
          const Spacer(),
          ChipBtn(
            label: context.t('common.remove'),
            tone: 'danger',
            onPressed: _busy
                ? null
                : () => _run(() async {
                      await backend.removePhone();
                      await refreshDetails();
                    }),
          ),
        ]),
        const SizedBox(height: 8),
        ToggleRow(
          padding: const EdgeInsets.symmetric(vertical: 8),
          label: context.t('settings.account.discoverable'),
          sub: context.t('settings.account.discoverableHint'),
          on: details.discoverableByPhone,
          onToggle: _busy
              ? null
              : () => _run(() async {
                    await backend.setDiscoverable(!details.discoverableByPhone);
                    await refreshDetails();
                  }),
        ),
      ]);
    } else {
      body = Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
        AppField(
          controller: _phone,
          label: context.t('settings.account.phoneNumber'),
          hint: '+40 721 000 000',
          keyboardType: TextInputType.phone,
          enabled: !_sent,
          onChanged: (_) => setState(() {}),
        ),
        const SizedBox(height: 10),
        if (!_sent)
          Btn(
            label: context.t('settings.account.sendCode'),
            kind: BtnKind.secondary,
            height: 44,
            busy: _busy,
            onPressed: digitsOnly(_phone.text).length < 7
                ? null
                : () => _run(() async {
                      await backend.requestPhoneCode(_phone.text.trim());
                      setState(() => _sent = true);
                    }),
          )
        else ...[
          AppField(
            controller: _code,
            label: context.t('settings.account.code'),
            description: context.t('settings.account.codeHint'),
            mono: true,
            keyboardType: TextInputType.number,
            maxLength: 8,
            inputFormatters: [FilteringTextInputFormatter.digitsOnly],
            onChanged: (_) => setState(() {}),
          ),
          const SizedBox(height: 10),
          Row(children: [
            Expanded(
              child: Btn(
                label: context.t('common.change'),
                kind: BtnKind.secondary,
                height: 44,
                onPressed: () => setState(() {
                  _sent = false;
                  _code.clear();
                }),
              ),
            ),
            const SizedBox(width: 8),
            Expanded(
              child: Btn(
                label: context.t('settings.account.verify'),
                height: 44,
                busy: _busy,
                onPressed: _code.text.length < 4
                    ? null
                    : () => _run(() async {
                          await backend.verifyPhone(phone: _phone.text.trim(), code: _code.text);
                          setState(() {
                            _sent = false;
                            _code.clear();
                          });
                          await refreshDetails();
                          notifyOk(tr('settings.account.phoneVerified'));
                        }),
              ),
            ),
          ]),
        ],
      ]);
    }
    return ListCard(children: [
      Padding(
        padding: const EdgeInsets.fromLTRB(16, 15, 16, 12),
        child: Row(crossAxisAlignment: CrossAxisAlignment.start, children: [
          Padding(padding: const EdgeInsets.only(top: 2), child: AppIcon('phone', size: 19, color: t.muted)),
          const SizedBox(width: 14),
          Expanded(
            child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
              Row(children: [
                Flexible(child: Text(context.t('settings.account.phone'), style: ts(14.5, weight: 600, color: t.ink))),
                const SizedBox(width: 8),
                SoftBadge(context.t('common.optional'), tone: 'muted'),
              ]),
              const SizedBox(height: 2),
              _Hint(context.t('settings.account.phoneHint')),
            ]),
          ),
        ]),
      ),
      Padding(
        padding: const EdgeInsets.all(16),
        child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
          body,
          if (_error != null) ...[const SizedBox(height: 10), Text(_error!, style: ts(13, color: t.danger))],
        ]),
      ),
    ]);
  }
}

// ---------------------------------------------------------------------------
// privacy
// ---------------------------------------------------------------------------

Future<void> _enableLock(BuildContext context, String accountId) async {
  final pw = TextEditingController();
  await showAppDialog<void>(context, title: context.t('lock.title'), builder: (ctx) {
    String? error;
    var busy = false;
    return StatefulBuilder(builder: (ctx, setState) {
      Future<void> submit() async {
        setState(() {
          busy = true;
          error = null;
        });
        try {
          if (!await backend.verifyPassword(pw.text)) {
            setState(() => error = tr('lock.wrongPassword'));
            return;
          }
          prefsNotifier.setLock(accountId, true);
          if (ctx.mounted) Navigator.of(ctx).pop();
          notifyOk(tr('lock.enabled'));
        } catch (e) {
          setState(() => error = errorText(e));
        } finally {
          if (ctx.mounted) setState(() => busy = false);
        }
      }

      return Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
        Text(ctx.t('lock.enableHint'), style: ts(13.5, color: ctx.tk.muted, height: 1.45)),
        const SizedBox(height: 14),
        AppField(
          controller: pw,
          label: ctx.t('password.label'),
          password: true,
          autofocus: true,
          error: error,
          onChanged: (_) => setState(() {}),
          onSubmitted: (_) => pw.text.isEmpty ? null : submit(),
        ),
        const SizedBox(height: 18),
        DialogActions(children: [
          Btn(label: ctx.t('common.cancel'), kind: BtnKind.secondary, height: 42, expand: false, onPressed: () => Navigator.of(ctx).pop()),
          Btn(label: ctx.t('lock.enable'), height: 42, expand: false, busy: busy, onPressed: pw.text.isEmpty ? null : submit),
        ]),
      ]);
    });
  });
}

String _relockKey(int minutes) => switch (minutes) { 1 => 'min1', 5 => 'min5', 60 => 'hour1', _ => 'start' };

Future<void> _chooseRelock(BuildContext context, int current) async {
  final v = await showOptions(
    context,
    title: context.t('settings.privacy.relock.title'),
    description: context.t('settings.privacy.relock.hint'),
    initial: '$current',
    options: [for (final m in kRelockChoicesMinutes) ('$m', context.t('settings.privacy.relock.${_relockKey(m)}'))],
  );
  if (v == null) return;
  prefsNotifier.update((x) => x.copyWith(relockAfterMinutes: int.parse(v)));
}

class _PrivacySection extends ConsumerWidget {
  const _PrivacySection();

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final privacy = ref.watch(storeProvider.select((s) => s.details?.privacy));
    final accountId = ref.watch(storeProvider.select((s) => s.details?.account.accountId));
    final p = ref.watch(prefsProvider);
    if (privacy == null) return const Spinner();
    Future<void> update({String? lastSeen, bool? readReceipts, bool? typing}) async {
      final res = await attempt(() => backend.setPrivacy(lastSeen: lastSeen, readReceipts: readReceipts, typingIndicators: typing));
      if (res != null) store.set((d) => d.copyWith(details: d.details?.copyWith(privacy: res)));
    }

    final lockOn = accountId != null && p.lockedAccounts.contains(accountId);
    return _Body(children: [
      ListCard(children: [
        ToggleRow(
          padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 15),
          label: context.t('settings.privacy.readReceipts'),
          sub: context.t('settings.privacy.readReceiptsHint'),
          on: privacy.readReceipts,
          onToggle: () => update(readReceipts: !privacy.readReceipts),
        ),
        ToggleRow(
          padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 15),
          label: context.t('settings.privacy.typing'),
          sub: context.t('settings.privacy.typingHint'),
          on: privacy.typingIndicators,
          onToggle: () => update(typing: !privacy.typingIndicators),
        ),
        ToggleRow(
          padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 15),
          label: context.t('settings.privacy.online'),
          sub: context.t('settings.privacy.onlineHint'),
          on: privacy.lastSeen == 'everyone',
          onToggle: () => update(lastSeen: privacy.lastSeen == 'everyone' ? 'nobody' : 'everyone'),
        ),
        ToggleRow(
          padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 15),
          label: context.t('settings.privacy.preview'),
          sub: context.t('settings.privacy.previewHint'),
          on: p.notifPreview,
          onToggle: () => prefsNotifier.update((x) => x.copyWith(notifPreview: !x.notifPreview)),
        ),
        ToggleRow(
          padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 15),
          label: context.t('settings.privacy.lock'),
          sub: lockOn
              ? '${context.t('settings.privacy.lockHint')} ${context.t('settings.privacy.relock.again.${_relockKey(p.relockAfterMinutes)}')}'
              : context.t('settings.privacy.lockHint'),
          on: lockOn,
          onToggle: accountId == null ? null : () => lockOn ? prefsNotifier.setLock(accountId, false) : _enableLock(context, accountId),
        ),
        if (lockOn)
          ListRow(
            title: context.t('settings.privacy.relock.title'),
            sub: context.t('settings.privacy.relock.${_relockKey(p.relockAfterMinutes)}'),
            onTap: () => _chooseRelock(context, p.relockAfterMinutes),
          ),
        ToggleRow(
          padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 15),
          label: context.t('mobile.screenSecurity'),
          sub: context.t(Platform.isIOS ? 'mobile.screenSecurityHintIos' : 'mobile.screenSecurityHint'),
          on: p.screenSecurity,
          onToggle: () => prefsNotifier.update((x) => x.copyWith(screenSecurity: !x.screenSecurity)),
        ),
        if (Platform.isAndroid)
          ToggleRow(
            padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 15),
            label: context.t('mobile.incognitoKeyboard'),
            sub: context.t('mobile.incognitoKeyboardHint'),
            on: p.incognitoKeyboard,
            onToggle: () => prefsNotifier.update((x) => x.copyWith(incognitoKeyboard: !x.incognitoKeyboard)),
          ),
        const _TemporaryModeRow(),
      ]),
      const _DefaultTimerCard(),
      const _MediaRetentionCard(),
      const _CallSettingsCard(),
      const VoiceMaskCard(),
      ListCard(children: [
        ListRow(
          icon: 'ban',
          title: context.t('mobile.blocked'),
          sub: context.t('mobile.blockedHint'),
          onTap: () => Navigator.of(context).push(MaterialPageRoute<void>(builder: (_) => const BlockedContactsPage())),
        ),
      ]),
    ]);
  }
}

/// Disappearing messages: the default timer of the chats we start and the groups we create
/// (synced to our devices; existing chats keep their own).
class _DefaultTimerCard extends StatelessWidget {
  const _DefaultTimerCard();

  Future<void> _choose(BuildContext context, int seconds) async {
    final v = await showOptions(
      context,
      title: context.t('defaultTimer.title'),
      description: context.t('defaultTimer.hint'),
      initial: '$seconds',
      options: [for (final (secs, key) in kDisappearingOptions) ('$secs', context.t('disappearing.$key'))],
    );
    if (v == null) return;
    await attempt(() => setDefaultTimer(int.parse(v)));
  }

  @override
  Widget build(BuildContext context) {
    return ValueListenableBuilder<int>(
      valueListenable: defaultTimer,
      builder: (context, seconds, _) => Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
        Padding(padding: const EdgeInsets.only(left: 4, bottom: 8), child: Eyebrow(context.t('disappearing.title'))),
        ListCard(children: [
          ListRow(title: context.t('defaultTimer.title'), sub: disappearingLabel(seconds, context.lang), onTap: () => _choose(context, seconds)),
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
            child: Text(context.t('defaultTimer.hint'), style: ts(13, color: context.tk.muted)),
          ),
        ]),
      ]),
    );
  }
}

/// Media: how long photos, videos, voice messages and files are kept (1, 7, 14 or 30 days, synced
/// to our devices, no "forever"), the last media check and "Check now".
class _MediaRetentionCard extends StatefulWidget {
  const _MediaRetentionCard();

  @override
  State<_MediaRetentionCard> createState() => _MediaRetentionCardState();
}

class _MediaRetentionCardState extends State<_MediaRetentionCard> {
  bool _checking = false;

  Future<void> _choose(MediaRetention r) async {
    final v = await showOptions(
      context,
      title: context.t('retention.title'),
      description: context.t('retention.hint'),
      initial: '${r.days}',
      options: [for (final d in r.options) ('$d', context.t('retention.days', count: d))],
    );
    if (v == null) return;
    await attempt(() => setMediaRetention(int.parse(v)));
  }

  Future<void> _checkNow() async {
    setState(() => _checking = true);
    await attempt(runMediaCheckNow);
    if (mounted) setState(() => _checking = false);
  }

  @override
  Widget build(BuildContext context) {
    final lang = context.lang;
    return ValueListenableBuilder<MediaRetention>(
      valueListenable: mediaRetention,
      builder: (context, r, _) {
        final last = r.lastCheck;
        final lastText = last == null
            ? context.t('retention.neverChecked')
            : context.t('retention.lastCheck', args: {'when': formatListTime(last.checkedAt, lang)}, count: last.removed);
        return Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
          Padding(padding: const EdgeInsets.only(left: 4, bottom: 8), child: Eyebrow(context.t('retention.section'))),
          ListCard(children: [
            ListRow(title: context.t('retention.title'), sub: context.t('retention.days', count: r.days), onTap: () => _choose(r)),
            Padding(
              padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
              child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
                Text(context.t('retention.hint'), style: ts(13, color: context.tk.muted)),
                const SizedBox(height: 6),
                Text(lastText, style: ts(13, color: context.tk.muted)),
                const SizedBox(height: 10),
                Btn(
                  label: context.t('retention.checkNow'),
                  kind: BtnKind.secondary,
                  height: 40,
                  busy: _checking,
                  onPressed: _checkNow,
                ),
              ]),
            ),
          ]),
        ]);
      },
    );
  }
}

/// Calls: allowed at all, and only through the server's TURN relay (synced to our devices).
class _CallSettingsCard extends StatelessWidget {
  const _CallSettingsCard();

  @override
  Widget build(BuildContext context) {
    return ValueListenableBuilder<CallSettings>(
      valueListenable: callSettings,
      builder: (context, c, _) => Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
        Padding(padding: const EdgeInsets.only(left: 4, bottom: 8), child: Eyebrow(context.t('settings.calls.title'))),
        ListCard(children: [
          ToggleRow(
            padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 15),
            label: context.t('settings.calls.allow'),
            sub: context.t('settings.calls.allowHint'),
            on: c.allowCalls,
            onToggle: () => unawaited(attempt(() => setCallSettings(allowCalls: !c.allowCalls))),
          ),
          // Greyed while calls are off: it only matters for calls.
          Opacity(
            opacity: c.allowCalls ? 1 : 0.5,
            child: ToggleRow(
              padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 15),
              label: context.t('settings.calls.relayOnly'),
              sub: context.t('settings.calls.relayOnlyHint'),
              on: c.relayOnly,
              onToggle: c.allowCalls ? () => unawaited(attempt(() => setCallSettings(relayOnly: !c.relayOnly))) : null,
            ),
          ),
          Consumer(
            builder: (context, ref, _) => ToggleRow(
              padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 15),
              label: context.t('voiceFx.denoiseSetting'),
              sub: context.t('mobile.voiceFx.denoiseHint'),
              on: ref.watch(prefsProvider.select((p) => p.callDenoise)),
              onToggle: () => prefsNotifier.update((x) => x.copyWith(callDenoise: !x.callDenoise)),
            ),
          ),
        ]),
      ]),
    );
  }
}

/// Temporary mode: chats work normally but nothing about them is kept on the device.
class _TemporaryModeRow extends StatefulWidget {
  const _TemporaryModeRow();

  @override
  State<_TemporaryModeRow> createState() => _TemporaryModeRowState();
}

class _TemporaryModeRowState extends State<_TemporaryModeRow> {
  bool? _on;

  @override
  void initState() {
    super.initState();
    unawaited(backend.getTemporaryMode().then((v) {
      if (mounted) setState(() => _on = v);
    }).catchError((Object _) {}));
  }

  Future<void> _toggle() async {
    final enable = !(_on ?? false);
    final ok = await showConfirm(
      context,
      title: context.t(enable ? 'temporary.enableTitle' : 'temporary.disableTitle'),
      body: context.t(enable ? 'temporary.enableBody' : 'temporary.disableBody'),
      confirmLabel: context.t(enable ? 'temporary.enable' : 'temporary.disable'),
    );
    if (!ok) return;
    final done = await attempt(() async {
      await backend.setTemporaryMode(enable);
      return true;
    });
    if (done == true && mounted) {
      setState(() => _on = enable);
      notifyOk(tr(enable ? 'temporary.enabled' : 'temporary.disabled'));
    }
  }

  @override
  Widget build(BuildContext context) => ToggleRow(
        padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 15),
        label: context.t('temporary.title'),
        sub: context.t('temporary.hint'),
        on: _on ?? false,
        onToggle: _on == null ? null : _toggle,
      );
}

class BlockedContactsPage extends ConsumerStatefulWidget {
  const BlockedContactsPage({super.key});

  @override
  ConsumerState<BlockedContactsPage> createState() => _BlockedContactsPageState();
}

class _BlockedContactsPageState extends ConsumerState<BlockedContactsPage> {
  List<Contact>? _list;

  @override
  void initState() {
    super.initState();
    _load();
  }

  Future<void> _load() async {
    final l = await attempt(() => backend.listBlocked());
    if (mounted) setState(() => _list = l ?? const []);
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    // Refresh when a contact is (un)blocked elsewhere.
    ref.listen(storeProvider.select((s) => s.contacts), (_, _) => _load());
    final list = _list;
    return PaneScaffold(
      header: PaneHeader(title: context.t('mobile.blocked')),
      body: list == null
          ? const Spinner()
          : ListView(padding: const EdgeInsets.symmetric(vertical: 8), children: [
              Padding(padding: const EdgeInsets.fromLTRB(20, 8, 20, 8), child: _Hint(context.t('mobile.blockedHint'))),
              if (list.isEmpty)
                Padding(
                  padding: const EdgeInsets.all(32),
                  child: Text(context.t('mobile.noBlocked'), textAlign: TextAlign.center, style: ts(14, color: t.muted)),
                ),
              for (final c in list)
                ContactRow(
                  c: c,
                  trailing: ChipBtn(
                    label: context.t('chat.block.unblock'),
                    tone: 'sunk',
                    onPressed: () async {
                      await setBlocked(c.accountId, false);
                      await _load();
                    },
                  ),
                ),
            ]),
    );
  }
}

// ---------------------------------------------------------------------------
// notifications
// ---------------------------------------------------------------------------

class _NotificationsSection extends ConsumerStatefulWidget {
  const _NotificationsSection();

  @override
  ConsumerState<_NotificationsSection> createState() => _NotificationsSectionState();
}

class _NotificationsSectionState extends ConsumerState<_NotificationsSection> {
  bool? _enabled;

  @override
  void initState() {
    super.initState();
    Notifications.instance.enabled().then((v) {
      if (mounted) setState(() => _enabled = v);
    });
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    final p = ref.watch(prefsProvider);
    Widget row(String label, String sub, bool on, VoidCallback onToggle) =>
        ToggleRow(padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 15), label: label, sub: sub, on: on, onToggle: onToggle);
    final dndOn = p.dndUntil > 0;
    return _Body(children: [
      ListCard(children: [
        row(context.t('dnd.title'), dndOn ? '${context.t('dnd.on')} · ${dndUntilText(p.dndUntil, context.lang)}' : context.t('dnd.hint'), dndOn,
            () => unawaited(dndOn ? attempt(() => setDnd(0)) : openDndDialog(context))),
        if (dndOn) ListRow(icon: 'moon', title: context.t('dnd.change'), onTap: () => unawaited(openDndDialog(context))),
      ]),
      ListCard(children: [
        row(context.t('mobile.notifications'), context.t('mobile.notificationsHint'), p.notifications, () async {
          if (!p.notifications) {
            final ok = await Notifications.instance.requestPermission();
            if (mounted) setState(() => _enabled = ok);
          }
          prefsNotifier.update((x) => x.copyWith(notifications: !x.notifications));
        }),
        row(context.t('settings.notifications.sound'), context.t('settings.notifications.soundHint'), p.sound,
            () => prefsNotifier.update((x) => x.copyWith(sound: !x.sound))),
        row(context.t('settings.notifications.soundInChat'), context.t('settings.notifications.soundInChatHint'), p.soundInChat,
            () => prefsNotifier.update((x) => x.copyWith(soundInChat: !x.soundInChat))),
        row(context.t('settings.privacy.preview'), context.t('settings.privacy.previewHint'), p.notifPreview,
            () => prefsNotifier.update((x) => x.copyWith(notifPreview: !x.notifPreview))),
      ]),
      if (p.notifications && _enabled == false) NoticeBox(tone: 'warn', icon: 'alert', text: context.t('mobile.notificationsDenied')),
      ListCard(children: [
        ListRow(
          icon: 'bell',
          title: context.t('mobile.push'),
          sub: context.t(Push.instance.available ? 'mobile.pushOn' : 'mobile.pushOff'),
          chevron: false,
        ),
        if (Platform.isAndroid)
          FutureBuilder<bool>(
            future: BatteryOptimization.isExempt(),
            builder: (context, snap) {
              final exempt = snap.data ?? true;
              return ListRow(
                icon: 'alert',
                title: context.t('mobile.battery.row'),
                sub: context.t(exempt ? 'mobile.battery.exempt' : 'mobile.battery.limited'),
                chevron: !exempt,
                onTap: exempt ? null : () => BatteryOptimization.request(),
              );
            },
          ),
      ]),
      Align(
        alignment: Alignment.centerLeft,
        child: Btn(
          label: context.t('settings.notifications.test'),
          icon: 'bell',
          kind: BtnKind.secondary,
          expand: false,
          height: 46,
          onPressed: () async {
            await Notifications.instance.requestPermission();
            await Notifications.instance.showSimple(kBrandName, tr('settings.notifications.testBody'));
          },
        ),
      ),
      Text(context.t('settings.notifications.mutedHint'), style: ts(12.5, color: t.muted, height: 1.5)),
    ]);
  }
}

// ---------------------------------------------------------------------------
// devices
// ---------------------------------------------------------------------------

class _DevicesSection extends ConsumerStatefulWidget {
  const _DevicesSection();

  @override
  ConsumerState<_DevicesSection> createState() => _DevicesSectionState();
}

class _DevicesSectionState extends ConsumerState<_DevicesSection> {
  List<DeviceInfo>? _devices;
  final _rename = TextEditingController();
  bool _editing = false;

  @override
  void initState() {
    super.initState();
    _load();
  }

  @override
  void dispose() {
    _rename.dispose();
    super.dispose();
  }

  Future<void> _load() async {
    try {
      final list = await backend.listDevices();
      list.sort((a, b) => a.current != b.current ? (a.current ? -1 : 1) : b.lastSeenAt.compareTo(a.lastSeenAt));
      if (mounted) setState(() => _devices = list);
    } catch (e) {
      notifyError(e);
    }
  }

  Future<void> _unlink(DeviceInfo d) async {
    final ok = await showConfirm(
      context,
      title: context.t('settings.devices.unlinkTitle', args: {'name': d.name}),
      body: context.t('settings.devices.unlinkBody'),
      confirmLabel: context.t('settings.devices.unlink'),
    );
    if (!ok) return;
    await attempt(() => backend.unlinkDevice(d.deviceId));
    await _load();
  }

  Future<void> _toggleApproval(bool enabled) async {
    final ok = await attempt(() async {
      await backend.setLoginApproval(enabled);
      return true;
    });
    if (ok == true) store.set((d) => d.copyWith(details: d.details?.copyWith(loginApproval: enabled)));
  }

  /// The server unlinks devices inactive for 30 days; warn a few days before.
  static int _inactiveDays(DeviceInfo d) => (DateTime.now().millisecondsSinceEpoch - d.lastSeenAt) ~/ 86400000;

  Future<void> _saveName() async {
    final n = _rename.text.trim();
    if (n.isEmpty) return;
    await attempt(() => backend.renameDevice(n));
    setState(() => _editing = false);
    await _load();
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    ref.listen(storeProvider.select((s) => s.devicesVersion), (_, _) => _load());
    final maxDevices = ref.watch(storeProvider.select((s) => s.serverInfo?.maxDevices));
    final loginApproval = ref.watch(storeProvider.select((s) => s.details?.loginApproval ?? false));
    final lang = context.lang;
    final devices = _devices;
    return _Body(children: [
      _Hint(context.t('settings.devices.intro'), size: 13.5),
      if (devices == null)
        const Spinner()
      else
        ListCard(children: [
          for (final d in devices)
            Padding(
              padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 15),
              child: Row(children: [
                Container(
                  width: 40,
                  height: 40,
                  decoration: BoxDecoration(color: t.sunk, borderRadius: BorderRadius.circular(12)),
                  child: Center(child: AppIcon('devices', size: 19, color: t.accentText)),
                ),
                const SizedBox(width: 14),
                Expanded(
                  child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
                    if (d.current && _editing)
                      SizedBox(
                        height: 40,
                        child: TextField(
                          enableIMEPersonalizedLearning: imePersonalizedLearning,
                          controller: _rename,
                          autofocus: true,
                          maxLength: 50,
                          onSubmitted: (_) => _saveName(),
                          style: ts(14.5, color: t.ink),
                          decoration: InputDecoration(
                            isDense: true,
                            counterText: '',
                            contentPadding: const EdgeInsets.symmetric(horizontal: 10, vertical: 10),
                            border: OutlineInputBorder(borderRadius: BorderRadius.circular(10), borderSide: BorderSide(color: t.line)),
                            suffixIcon: Row(mainAxisSize: MainAxisSize.min, children: [
                              IconBtn('check', size: 30, iconSize: 16, color: t.accentText, tooltip: context.t('common.save'), onPressed: _saveName),
                              IconBtn('x', size: 30, iconSize: 16, color: t.muted, tooltip: context.t('common.cancel'), onPressed: () => setState(() => _editing = false)),
                            ]),
                          ),
                        ),
                      )
                    else
                      Row(children: [
                        Flexible(child: Text(d.name, maxLines: 1, overflow: TextOverflow.ellipsis, style: ts(14.5, weight: 600, color: t.ink))),
                        if (d.current) ...[const SizedBox(width: 8), SoftBadge(context.t('settings.devices.thisDevice'))],
                      ]),
                    Text(
                      d.current
                          ? context.t('settings.devices.activeNow', args: {'date': formatDateTime(d.createdAt, lang)})
                          : context.t('settings.devices.lastActive', args: {'date': formatDateTime(d.lastSeenAt, lang)}),
                      style: ts(12.5, color: t.muted),
                    ),
                    if (!d.current && _inactiveDays(d) >= 25)
                      Text(
                        context.t('settings.devices.inactiveWarning', args: {'days': '${_inactiveDays(d)}'}),
                        style: ts(12.5, color: t.danger),
                      ),
                  ]),
                ),
                const SizedBox(width: 8),
                if (d.current)
                  (_editing
                      ? const SizedBox.shrink()
                      : IconBtn('edit', iconSize: 17, color: t.muted, tooltip: context.t('settings.devices.rename'), onPressed: () {
                          _rename.text = d.name;
                          setState(() => _editing = true);
                        }))
                else
                  ChipBtn(label: context.t('settings.devices.unlink'), tone: 'danger', onPressed: () => _unlink(d)),
              ]),
            ),
        ]),
      ListCard(children: [
        ToggleRow(
          icon: 'shield',
          label: context.t('settings.devices.approvalLabel'),
          sub: context.t('settings.devices.approvalHint'),
          on: loginApproval,
          onToggle: () => _toggleApproval(!loginApproval),
        ),
      ]),
      if (maxDevices != null && maxDevices > 0) _Hint(context.t('settings.devices.descriptionMax', count: maxDevices)),
      Btn(
        label: context.t('settings.devices.link'),
        icon: 'scan',
        height: 48,
        onPressed: () => showLinkDevice(context, newDevice: false, onLinked: _load),
      ),
    ]);
  }
}

// ---------------------------------------------------------------------------
// chats: backup + address book import
// ---------------------------------------------------------------------------

class _ChatsSection extends ConsumerStatefulWidget {
  const _ChatsSection();

  @override
  ConsumerState<_ChatsSection> createState() => _ChatsSectionState();
}

class _ChatsSectionState extends ConsumerState<_ChatsSection> {
  bool _busy = false;
  ContactImportResult? _result;

  Future<void> _toggleBackup(bool enabled) async {
    final ok = await attempt(() async {
      await backend.setBackupEnabled(enabled);
      return true;
    });
    if (ok == true) store.set((d) => d.copyWith(details: d.details?.copyWith(backupEnabled: enabled)));
  }

  Future<void> _import(String path) async {
    setState(() {
      _busy = true;
      _result = null;
    });
    final res = await attempt(() => backend.importContacts(path));
    if (!mounted) return;
    setState(() {
      _busy = false;
      _result = res;
    });
    if (res != null) unawaited(reloadContacts());
  }

  Future<void> _importFile() async {
    final files = await pickFiles(kind: 'contacts');
    try {
      if (files.isNotEmpty) await _import(files.first.path);
    } finally {
      discardPicked(files);
    }
  }

  Future<void> _importPhone() async {
    final path = await Navigator.of(context).push<String>(MaterialPageRoute(builder: (_) => const _PhoneContactsPage()));
    if (path != null) {
      try {
        await _import(path);
      } finally {
        // The exported address book never stays on disk.
        unawaited(deleteTempCopy(path));
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    final backup = ref.watch(storeProvider.select((s) => s.details?.backupEnabled ?? false));
    final r = _result;
    return _Body(children: [
      _SubLabel(context.t('settings.chats.backup')),
      ListCard(children: [
        ToggleRow(
          icon: 'lock',
          label: context.t('settings.chats.backupLabel'),
          sub: context.t('settings.chats.backupHint'),
          on: backup,
          onToggle: () => _toggleBackup(!backup),
        ),
      ]),
      _SubLabel(context.t('settings.chats.import')),
      ListCard(children: [
        Padding(
          padding: const EdgeInsets.all(16),
          child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
            _Hint(context.t('settings.chats.importHint'), size: 13.5),
            const SizedBox(height: 12),
            if (isMobilePlatform) ...[
              Btn(label: context.t('mobile.importPhone'), icon: 'book', height: 44, busy: _busy, onPressed: _importPhone),
              const SizedBox(height: 8),
            ],
            Btn(
              label: isMobilePlatform ? context.t('mobile.importFile') : context.t('settings.chats.importButton'),
              icon: 'file',
              kind: BtnKind.secondary,
              height: 44,
              busy: _busy && !isMobilePlatform,
              onPressed: _busy ? null : _importFile,
            ),
            if (r != null) ...[
              const SizedBox(height: 12),
              NoticeBox(
                icon: 'okcircle',
                text: context.t('settings.chats.importResult', args: {'total': r.total, 'withPhone': r.withPhone, 'matched': r.matched}),
              ),
            ],
            const SizedBox(height: 12),
            Text(context.t('settings.chats.importPrivacy'), style: ts(12.5, color: t.muted)),
          ]),
        ),
      ]),
      const SizedBox(height: 16),
      Btn(
        label: context.t('wipe.action'),
        icon: 'trash',
        kind: BtnKind.danger,
        height: 44,
        onPressed: () => _wipeAll(context),
      ),
    ]);
  }
}

/// Deletes every conversation, here and for the other side of each one (password required).
Future<void> _wipeAll(BuildContext context) async {
  final pw = TextEditingController();
  await showAppDialog<void>(context, title: context.t('wipe.title'), builder: (ctx) {
    String? error;
    var busy = false;
    return StatefulBuilder(builder: (ctx, setState) {
      Future<void> submit() async {
        setState(() {
          busy = true;
          error = null;
        });
        try {
          if (!await backend.verifyPassword(pw.text)) {
            setState(() => error = tr('wipe.wrongPassword'));
            return;
          }
          await backend.wipeAllConversations(pw.text);
          await reloadConversations();
          if (ctx.mounted) Navigator.of(ctx).pop();
          notifyOk(tr('wipe.done'));
        } catch (e) {
          setState(() => error = errorText(e));
        } finally {
          if (ctx.mounted) setState(() => busy = false);
        }
      }

      final hint = ts(13.5, color: ctx.tk.muted, height: 1.45);
      return Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
        Text(ctx.t('wipe.body'), style: hint),
        const SizedBox(height: 8),
        for (final k in ['wipe.point1', 'wipe.point2', 'wipe.point3']) Text('• ${ctx.t(k)}', style: hint),
        const SizedBox(height: 14),
        AppField(
          controller: pw,
          label: ctx.t('password.label'),
          password: true,
          error: error,
          onChanged: (_) => setState(() {}),
          onSubmitted: (_) => pw.text.isEmpty ? null : submit(),
        ),
        const SizedBox(height: 18),
        DialogActions(children: [
          Btn(label: ctx.t('common.cancel'), kind: BtnKind.secondary, height: 42, expand: false, onPressed: () => Navigator.of(ctx).pop()),
          Btn(label: ctx.t('wipe.confirm'), kind: BtnKind.danger, height: 42, expand: false, busy: busy, onPressed: pw.text.isEmpty ? null : submit),
        ]),
      ]);
    });
  });
}

/// Picks contacts from the phone's address book and returns the path of a temporary .vcf file.
class _PhoneContactsPage extends StatefulWidget {
  const _PhoneContactsPage();

  @override
  State<_PhoneContactsPage> createState() => _PhoneContactsPageState();
}

class _PhoneContactsPageState extends State<_PhoneContactsPage> {
  List<fc.Contact>? _contacts;
  final _selected = <String>{};
  String? _error;
  final _query = TextEditingController();

  @override
  void initState() {
    super.initState();
    _load();
  }

  @override
  void dispose() {
    _query.dispose();
    super.dispose();
  }

  Future<void> _load() async {
    try {
      final status = await fc.FlutterContacts.permissions.request(fc.PermissionType.read);
      if (status != fc.PermissionStatus.granted && status != fc.PermissionStatus.limited) {
        setState(() => _error = tr('mobile.contactsDenied'));
        return;
      }
      final all = await fc.FlutterContacts.getAll(properties: {fc.ContactProperty.name, fc.ContactProperty.phone});
      final withPhone = all.where((c) => c.phones.isNotEmpty).toList()
        ..sort((a, b) => (a.displayName ?? '').toLowerCase().compareTo((b.displayName ?? '').toLowerCase()));
      setState(() {
        _contacts = withPhone;
        _selected.addAll(withPhone.map((c) => c.id ?? '').where((id) => id.isNotEmpty));
      });
    } catch (e) {
      setState(() => _error = errorText(e));
    }
  }

  Future<void> _done() async {
    final chosen = (_contacts ?? const <fc.Contact>[]).where((c) => _selected.contains(c.id)).toList();
    final vcf = fc.FlutterContacts.vCard.exportAll(chosen);
    final f = File(await appTempPath('contacts.vcf'));
    await f.writeAsString(vcf, flush: true);
    if (mounted) Navigator.of(context).pop(f.path);
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    final list = _contacts;
    final q = _query.text.trim().toLowerCase();
    final shown = list?.where((c) => q.isEmpty || (c.displayName ?? '').toLowerCase().contains(q)).toList();
    return PaneScaffold(
      header: PaneHeader(
        title: context.t('mobile.pickContacts'),
        backIcon: 'x',
        actions: [
          if (list != null && list.isNotEmpty)
            LinkBtn(
              label: context.t('mobile.selectAll'),
              fontSize: 13,
              onPressed: () => setState(() {
                if (_selected.length == list.length) {
                  _selected.clear();
                } else {
                  _selected.addAll(list.map((c) => c.id ?? ''));
                }
              }),
            ),
        ],
      ),
      body: _error != null
          ? Padding(padding: const EdgeInsets.all(24), child: NoticeBox(tone: 'warn', icon: 'alert', text: _error!))
          : shown == null
              ? const Spinner()
              : Column(children: [
                  Padding(
                    padding: const EdgeInsets.fromLTRB(16, 12, 16, 8),
                    child: Text(context.t('mobile.importPhoneHint'), style: ts(12.5, color: t.muted, height: 1.45)),
                  ),
                  Expanded(
                    child: ListView.builder(
                      itemCount: shown.length,
                      itemBuilder: (_, i) {
                        final c = shown[i];
                        final on = _selected.contains(c.id);
                        return PickRow(
                          leading: Avatar(name: c.displayName ?? '?', seed: c.id ?? '$i', size: 40),
                          title: c.displayName ?? c.phones.first.number,
                          sub: c.phones.map((p) => p.number).join(', '),
                          onTap: () => setState(() => on ? _selected.remove(c.id) : _selected.add(c.id ?? '')),
                          trailing: CheckBox(on: on, round: true),
                        );
                      },
                    ),
                  ),
                ]),
      bottom: Container(
        padding: EdgeInsets.fromLTRB(16, 12, 16, 12 + MediaQuery.of(context).padding.bottom),
        decoration: BoxDecoration(color: t.pane, border: Border(top: BorderSide(color: t.line))),
        child: Btn(
          label: context.t('mobile.importSelected', count: _selected.length),
          height: 48,
          onPressed: _selected.isEmpty ? null : _done,
        ),
      ),
    );
  }
}

// ---------------------------------------------------------------------------
// appearance
// ---------------------------------------------------------------------------

/// Theme, style and text size (Settings → Appearance, and the first-run setup).
class AppearanceSection extends ConsumerWidget {
  const AppearanceSection({super.key, this.embedded = false});

  /// Inside another scrolling page (the first-run setup).
  final bool embedded;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final t = context.tk;
    final p = ref.watch(prefsProvider);
    final light = ChatTokens.build(p.direction, dark: false);
    final dark = ChatTokens.build(p.direction, dark: true);
    Widget ring(bool on, Widget child) => Container(
          decoration: BoxDecoration(
            color: t.pane,
            borderRadius: BorderRadius.circular(t.rCard),
            border: Border.all(color: on ? t.accent : t.line, width: on ? 2 : 1),
          ),
          child: child,
        );
    Widget bubble(ChatTokens x, bool out, double widthFactor) => FractionallySizedBox(
          widthFactor: widthFactor,
          child: Container(
            height: 14,
            decoration: BoxDecoration(
              color: out ? (x.bubbleOutGradient == null ? x.bubbleOut : null) : (x.bubbleIn == Colors.white ? const Color(0xFFE4DFD4) : x.bubbleIn),
              gradient: out ? x.bubbleOutGradient : null,
              borderRadius: BorderRadius.circular(7),
            ),
          ),
        );
    Widget themeCard(String value, String label) {
      final on = p.theme == value;
      final pal = value == 'dark' ? dark : light;
      return Expanded(
        child: GestureDetector(
          onTap: () => ref.read(prefsProvider.notifier).setTheme(value),
          child: ring(
            on,
            Padding(
              padding: const EdgeInsets.all(10),
              child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
                Container(
                  height: 86,
                  padding: const EdgeInsets.all(12),
                  decoration: BoxDecoration(
                    borderRadius: BorderRadius.circular((t.rCard - 6).clamp(4, 40)),
                    color: value == 'auto' ? null : pal.chatBg,
                    gradient: value == 'auto'
                        ? LinearGradient(colors: [light.chatBg, light.chatBg, dark.chatBg, dark.chatBg], stops: const [0, .5, .5, 1])
                        : null,
                  ),
                  child: Column(mainAxisAlignment: MainAxisAlignment.end, children: [
                    Align(alignment: Alignment.centerLeft, child: bubble(value == 'auto' ? light : pal, false, .58)),
                    const SizedBox(height: 6),
                    Align(alignment: Alignment.centerRight, child: bubble(value == 'auto' ? dark : pal, true, .46)),
                  ]),
                ),
                const SizedBox(height: 10),
                Padding(padding: const EdgeInsets.symmetric(horizontal: 4), child: Text(label, style: ts(14, weight: 650, color: t.ink))),
              ]),
            ),
          ),
        ),
      );
    }

    Widget dirCard(Direction d, String label, String sub) => Expanded(
          child: GestureDetector(
            onTap: () => ref.read(prefsProvider.notifier).setDirection(d),
            child: ring(
              p.direction == d,
              Padding(
                padding: const EdgeInsets.all(14),
                child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
                  Text(label, style: ts(14.5, weight: 700, color: t.ink)),
                  const SizedBox(height: 4),
                  Text(sub, style: ts(12.5, color: t.muted, height: 1.4)),
                ]),
              ),
            ),
          ),
        );

    return _Body(embedded: embedded, children: [
      _SubLabel(context.t('settings.appearance.theme')),
      Row(children: [
        themeCard('light', context.t('settings.appearance.light')),
        const SizedBox(width: 10),
        themeCard('dark', context.t('settings.appearance.dark')),
        const SizedBox(width: 10),
        themeCard('auto', context.t('settings.appearance.auto')),
      ]),
      _SubLabel(context.t('settings.appearance.style')),
      IntrinsicHeight(
        child: Row(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
          dirCard(Direction.a, context.t('settings.appearance.dirA'), context.t('settings.appearance.dirAHint')),
          const SizedBox(width: 10),
          dirCard(Direction.b, context.t('settings.appearance.dirB'), context.t('settings.appearance.dirBHint')),
        ]),
      ),
      Row(children: [
        Expanded(child: _SubLabel(context.t('mobile.fontSize'))),
        Text('${(p.textScale * 100).round()}%', style: ts(13, weight: 600, color: t.muted, mono: true)),
        if (p.textScale != 1) ...[
          const SizedBox(width: 10),
          LinkBtn(label: context.t('mobile.fontReset'), fontSize: 13, onPressed: () => ref.read(prefsProvider.notifier).setTextScale(1)),
        ],
      ]),
      ring(
        false,
        Padding(
          padding: const EdgeInsets.fromLTRB(12, 12, 12, 4),
          child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
            Text(context.t('mobile.fontPreview'), style: ts(15, color: t.ink, height: 1.42)),
            Row(children: [
              Text('A', style: ts(13, weight: 600, color: t.muted)),
              Expanded(
                child: Slider(
                  value: p.textScale,
                  min: kMinTextScale,
                  max: kMaxTextScale,
                  divisions: 9,
                  activeColor: t.accent,
                  inactiveColor: t.line,
                  label: '${(p.textScale * 100).round()}%',
                  onChanged: (v) => ref.read(prefsProvider.notifier).setTextScale(double.parse(v.toStringAsFixed(2))),
                ),
              ),
              Text('A', style: ts(20, weight: 600, color: t.muted)),
            ]),
          ]),
        ),
      ),
    ]);
  }
}

// ---------------------------------------------------------------------------
// language
// ---------------------------------------------------------------------------

class _LanguageSection extends ConsumerWidget {
  const _LanguageSection();

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final t = context.tk;
    final current = context.lang;
    final sys = systemLanguage();
    return _Body(children: [
      ListCard(children: [
        for (final l in kLanguages)
          Tap(
            onTap: () => ref.read(prefsProvider.notifier).setLanguage(l.code),
            padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 15),
            child: Row(children: [
              Expanded(
                child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
                  Text(l.label, style: ts(14.5, weight: 600, color: t.ink)),
                  Text(sys == l.code ? context.t('settings.language.system') : context.t('settings.language.names.${l.code}'), style: ts(12.5, color: t.muted)),
                ]),
              ),
              RadioDot(on: current == l.code),
            ]),
          ),
      ]),
      _Hint(context.t('settings.language.hint')),
    ]);
  }
}

// ---------------------------------------------------------------------------
// about
// ---------------------------------------------------------------------------

class _AboutSection extends ConsumerWidget {
  const _AboutSection();

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final t = context.tk;
    final server = ref.watch(storeProvider.select((s) => s.serverInfo));
    final serverUrl = ref.watch(storeProvider.select((s) => s.details?.account.serverUrl));
    return _Body(children: [
      Padding(
        padding: const EdgeInsets.symmetric(vertical: 18),
        child: Column(children: [
          Container(
            width: 64,
            height: 64,
            decoration: BoxDecoration(color: t.accent, borderRadius: BorderRadius.circular(19)),
            child: Center(child: AppIcon('chat', size: 32, color: t.onAccent)),
          ),
          const SizedBox(height: 10),
          Text(kBrandName, style: ts(20, weight: 750, color: t.ink)),
          const SizedBox(height: 4),
          Text(context.t('settings.about.version', args: {'version': kAppVersion}), style: ts(13, color: t.muted, mono: true)),
          const SizedBox(height: 10),
          Text(context.t('mobile.aboutTagline'), textAlign: TextAlign.center, style: ts(13.5, color: t.muted, height: 1.5)),
        ]),
      ),
      ListCard(children: [
        ListRow(
          icon: 'server',
          title: server?.name ?? context.t('settings.about.server'),
          sub: [server?.url ?? serverUrl ?? '', if (server != null) context.t('onboarding.server.version', args: {'version': server.version})].join('\n'),
          chevron: false,
        ),
        ListRow(icon: 'shield', title: context.t('settings.about.security'), sub: context.t('settings.about.signal'), chevron: false),
        ListRow(
          icon: 'file',
          title: context.t('settings.about.license'),
          sub: context.t('settings.about.openSource'),
          trailing: AppIcon('external', size: 16, color: t.faint),
          onTap: () => launchUrl(Uri.parse('https://www.gnu.org/licenses/agpl-3.0.html'), mode: LaunchMode.externalApplication),
        ),
      ]),
    ]);
  }
}
