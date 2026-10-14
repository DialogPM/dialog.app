// Username helpers (Signal-style `nickname.01` discriminators) and the username link / QR
// dialog. The link carries no account number; scanning it in "Add contact" adds the account.

import 'dart:math';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:share_plus/share_plus.dart';

import '../../app/globals.dart';
import '../../state/actions.dart';
import '../../core/models.dart';
import '../../l10n/i18n.dart';
import '../../theme/app_theme.dart';
import '../../widgets/dialogs.dart';
import '../../widgets/qr.dart';
import '../../widgets/ui.dart';

final _discriminatorRe = RegExp(r'\.([0-9]+)$');

/// The trailing `.<digits>` of a username, or null.
String? usernameDiscriminator(String u) => _discriminatorRe.firstMatch(u)?.group(1);

/// Server rules for a trailing number: 2-9 digits, not all zeros, a leading 0 only in the
/// 2-digit form. Usernames without a trailing number are fine (and old ones keep working).
bool usernameDiscriminatorValid(String u) {
  final d = usernameDiscriminator(u);
  if (d == null) return true;
  return d.length >= 2 && d.length <= 9 && d.contains(RegExp('[1-9]')) && !(d.length > 2 && d.startsWith('0'));
}

/// [name] with a fresh random 2-digit discriminator (replacing an existing one).
String withRandomDiscriminator(String name) {
  final d = usernameDiscriminator(name);
  var nick = d == null ? name : name.substring(0, name.length - d.length - 1);
  if (nick.length > 29) nick = nick.substring(0, 29);
  final n = Random.secure().nextInt(99) + 1;
  return '$nick.${n.toString().padLeft(2, '0')}';
}

/// Shows the username link as a QR code with copy / share / reset.
Future<void> showUsernameLink(BuildContext context) async {
  await showAppDialog<void>(context, title: context.t('settings.username.linkTitle'), builder: (ctx) => const _UsernameLinkBody());
}

class _UsernameLinkBody extends StatefulWidget {
  const _UsernameLinkBody();

  @override
  State<_UsernameLinkBody> createState() => _UsernameLinkBodyState();
}

class _UsernameLinkBodyState extends State<_UsernameLinkBody> {
  UsernameLink? _link;
  String? _error;
  bool _busy = true;

  @override
  void initState() {
    super.initState();
    _load(backend.getUsernameLink);
  }

  Future<void> _load(Future<UsernameLink?> Function() f, {bool reset = false}) async {
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      final l = await f();
      if (!mounted) return;
      setState(() {
        _link = l;
        _busy = false;
      });
      if (reset) notifyOk(tr('settings.username.linkResetDone'));
    } catch (e) {
      if (!mounted) return;
      setState(() {
        _busy = false;
        _error = errorText(e);
      });
    }
  }

  Future<void> _reset() async {
    final ok = await showConfirm(
      context,
      title: context.t('settings.username.linkReset'),
      body: context.t('settings.username.linkResetConfirm'),
      confirmLabel: context.t('settings.username.linkReset'),
    );
    if (ok) await _load(backend.resetUsernameLink, reset: true);
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    final link = _link;
    return Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
      Center(
        child: link != null
            ? QrCodeView(data: link.link, size: 190)
            : SizedBox(height: 120, child: _busy ? const Spinner() : null),
      ),
      if (link != null) ...[
        const SizedBox(height: 10),
        Text('@${link.username}', textAlign: TextAlign.center, style: ts(15, weight: 650, color: t.ink)),
      ],
      const SizedBox(height: 10),
      Text(context.t('settings.username.linkHint'), textAlign: TextAlign.center, style: ts(13, color: t.muted, height: 1.45)),
      if (_error != null) ...[
        const SizedBox(height: 8),
        Text(_error!, textAlign: TextAlign.center, style: ts(13, color: t.danger)),
      ],
      const SizedBox(height: 14),
      Wrap(alignment: WrapAlignment.center, spacing: 8, runSpacing: 8, children: [
        ChipBtn(
          label: context.t('qr.copyLink'),
          icon: 'copy',
          onPressed: link == null
              ? null
              : () async {
                  await Clipboard.setData(ClipboardData(text: link.link));
                  notifyOk(tr('qr.linkCopied'));
                },
        ),
        ChipBtn(
          label: context.t('qr.share'),
          icon: 'share',
          onPressed: link == null ? null : () => SharePlus.instance.share(ShareParams(text: link.link, subject: context.t('qr.shareTitle'))),
        ),
        ChipBtn(label: context.t('settings.username.linkReset'), icon: 'refresh', tone: 'danger', onPressed: _busy || link == null ? null : _reset),
      ]),
    ]);
  }
}
