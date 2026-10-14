// "QR code" modal: my contact code (copy / share) and the scanner to add a contact.

import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';
import 'package:share_plus/share_plus.dart';

import '../../app/globals.dart';
import '../../app/nav.dart';
import '../../core/backend.dart';
import '../../l10n/i18n.dart';
import '../../state/actions.dart';
import '../../state/store.dart';
import '../../theme/app_theme.dart';
import '../../util/format.dart';
import '../../widgets/dialogs.dart';
import '../../widgets/qr.dart';
import '../../widgets/seg_tabs.dart';
import '../../widgets/ui.dart';

Future<void> showQrSheet(BuildContext context, {String initial = 'mine'}) =>
    showAppDialog<void>(context, title: context.t('qr.title'), builder: (ctx) => _QrTabs(initial: initial));

class _QrTabs extends StatefulWidget {
  const _QrTabs({required this.initial});
  final String initial;

  @override
  State<_QrTabs> createState() => _QrTabsState();
}

class _QrTabsState extends State<_QrTabs> {
  late String _tab = widget.initial;

  @override
  Widget build(BuildContext context) {
    return Column(crossAxisAlignment: CrossAxisAlignment.stretch, mainAxisSize: MainAxisSize.min, children: [
      SegTabs(
        tabs: [('mine', 'qr', context.t('qr.myCodeShort')), ('scan', 'scan', context.t('qr.scanShort'))],
        value: _tab,
        onChanged: (v) => setState(() => _tab = v),
      ),
      const SizedBox(height: 16),
      if (_tab == 'mine') const MyQrCode() else const _ScanContact(),
    ]);
  }
}

class MyQrCode extends ConsumerStatefulWidget {
  const MyQrCode({super.key});

  @override
  ConsumerState<MyQrCode> createState() => _MyQrCodeState();
}

class _MyQrCodeState extends ConsumerState<MyQrCode> {
  String? _link;
  bool _copied = false;

  @override
  void initState() {
    super.initState();
    backend.getMyContactLink().then((l) {
      if (mounted) setState(() => _link = l);
    }).catchError((Object _) {});
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    final number = ref.watch(storeProvider.select((s) => s.details?.account.number ?? ''));
    final name = ref.watch(storeProvider.select((s) => s.profile?.name ?? ''));
    final link = _link;
    return Column(mainAxisSize: MainAxisSize.min, crossAxisAlignment: CrossAxisAlignment.stretch, children: [
      const SizedBox(height: 6),
      Center(child: link != null ? QrCodeView(data: link) : const SizedBox(height: 250, child: Spinner())),
      const SizedBox(height: 14),
      if (name.isNotEmpty) Text(name, textAlign: TextAlign.center, style: ts(16, weight: 700, color: t.ink)),
      const SizedBox(height: 4),
      Text(formatAccountNumber(number), textAlign: TextAlign.center, style: ts(17, weight: 600, color: t.ink, mono: true, letterSpacing: 1)),
      const SizedBox(height: 14),
      Text(context.t('qr.myCodeHint'), textAlign: TextAlign.center, style: ts(13, color: t.muted, height: 1.45)),
      if (link != null) ...[
        const SizedBox(height: 14),
        Row(children: [
          Expanded(
            child: Btn(
              label: context.t(_copied ? 'qr.linkCopied' : 'qr.copyLink'),
              icon: _copied ? 'check' : 'copy',
              kind: BtnKind.soft,
              height: 44,
              onPressed: () async {
                await Clipboard.setData(ClipboardData(text: link));
                setState(() => _copied = true);
                Timer(const Duration(milliseconds: 1600), () => mounted ? setState(() => _copied = false) : null);
              },
            ),
          ),
          const SizedBox(width: 8),
          Expanded(
            child: Btn(
              label: context.t('qr.share'),
              icon: 'share',
              kind: BtnKind.secondary,
              height: 44,
              onPressed: () => SharePlus.instance.share(ShareParams(text: link, subject: context.t('qr.shareTitle'))),
            ),
          ),
        ]),
      ],
    ]);
  }
}

class _ScanContact extends StatefulWidget {
  const _ScanContact();

  @override
  State<_ScanContact> createState() => _ScanContactState();
}

class _ScanContactState extends State<_ScanContact> {
  bool _busy = false;
  String? _error;

  Future<void> _handle(String link) async {
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      final c = await backend.addContactFromLink(link);
      store.upsertContact(c);
      if (mounted) Navigator.of(context).pop();
      await openDirect(c.accountId);
    } catch (e) {
      final code = parseError(e).code;
      if (mounted) setState(() => _error = code == 'different_server' ? tr('qr.differentServer') : errorText(e));
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    return Column(mainAxisSize: MainAxisSize.min, crossAxisAlignment: CrossAxisAlignment.stretch, children: [
      QrScanner(onResult: _handle, busy: _busy),
      if (_error != null) ...[
        const SizedBox(height: 10),
        Text(_error!, textAlign: TextAlign.center, style: ts(13, color: context.tk.danger)),
      ],
    ]);
  }
}
