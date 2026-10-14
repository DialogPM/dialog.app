// Device linking by QR code, in the design's QR modal ("My code" / "Scan" tabs).
//  - mode "new": this device is signed out and joins an existing account (log-in screen).
//  - mode "existing": this device is signed in and approves a new one (Settings -> Devices).

import 'dart:async';

import 'package:flutter/material.dart';

import '../../app/globals.dart';
import '../../core/backend.dart';
import '../../l10n/i18n.dart';
import '../../services/security.dart';
import '../../state/actions.dart';
import '../../theme/app_theme.dart';
import '../../widgets/app_icon.dart';
import '../../widgets/dialogs.dart';
import '../../widgets/qr.dart';
import '../../widgets/seg_tabs.dart';
import '../../widgets/ui.dart';

Future<void> showLinkDevice(BuildContext context, {required bool newDevice, String? serverUrl, VoidCallback? onLinked}) async {
  await showAppDialog<void>(
    context,
    title: context.t(newDevice ? 'link.titleNew' : 'link.titleExisting'),
    builder: (ctx) => _LinkFlow(newDevice: newDevice, serverUrl: serverUrl, onLinked: onLinked),
  );
  unawaited(backend.linkCancel().catchError((_) {}));
}

sealed class _Phase {
  const _Phase();
}

class _Idle extends _Phase {
  const _Idle();
}

class _Busy extends _Phase {
  const _Busy();
}

class _Code extends _Phase {
  const _Code(this.code);
  final String code;
}

class _Waiting extends _Phase {
  const _Waiting([this.sas]);
  final String? sas;
}

class _Confirm extends _Phase {
  const _Confirm(this.name, [this.link, this.sas]);
  final String name;
  final String? link;
  final String? sas;
}

class _Expired extends _Phase {
  const _Expired();
}

class _Failed extends _Phase {
  const _Failed(this.message);
  final String message;
}

class _LinkFlow extends StatefulWidget {
  const _LinkFlow({required this.newDevice, this.serverUrl, this.onLinked});

  final bool newDevice;
  final String? serverUrl;
  final VoidCallback? onLinked;

  @override
  State<_LinkFlow> createState() => _LinkFlowState();
}

class _LinkFlowState extends State<_LinkFlow> {
  String _tab = 'scan';
  _Phase _phase = const _Idle();

  // Every started flow gets a number; results of an older flow (tab switched) are ignored.
  int _session = 0;

  void _set(_Phase p) {
    if (mounted) setState(() => _phase = p);
  }

  void _fail(int id, Object e) {
    if (_session != id) return;
    final err = parseError(e);
    if (err.code == 'gone') {
      _set(const _Expired());
    } else if (err.code == 'different_server') {
      _set(_Failed(tr('link.differentServer')));
    } else {
      _set(_Failed(errorText(e)));
    }
  }

  void _close() {
    if (mounted) Navigator.of(context).pop();
  }

  Future<void> _finishNew(int id) async {
    await backend.linkWait();
    if (_session != id) return;
    _close();
    await enterApp();
  }

  Future<void> _startMine() async {
    final id = ++_session;
    _set(const _Busy());
    try {
      if (widget.newDevice) {
        final code = await backend.linkCreate(serverUrl: widget.serverUrl ?? '', deviceName: defaultDeviceName());
        if (_session != id) return;
        _set(_Code(code));
        await _finishNew(id);
      } else {
        final code = await backend.linkOffer();
        if (_session != id) return;
        _set(_Code(code));
        final name = await backend.linkOfferWait();
        final sas = await backend.linkCode();
        if (_session != id) return;
        _set(_Confirm(name, null, sas));
      }
    } catch (e) {
      _fail(id, e);
    }
  }

  Future<void> _onScanned(String link) async {
    final id = ++_session;
    _set(const _Busy());
    try {
      if (widget.newDevice) {
        await backend.linkClaim(link: link, deviceName: defaultDeviceName());
        final sas = await backend.linkCode();
        if (_session != id) return;
        _set(_Waiting(sas));
        await _finishNew(id);
      } else {
        final name = await backend.linkInspect(link);
        if (_session != id) return;
        _set(_Confirm(name, link));
      }
    } catch (e) {
      _fail(id, e);
    }
  }

  Future<void> _approve(String? link) async {
    final id = ++_session;
    _set(const _Busy());
    try {
      await backend.linkApprove(link: link);
      if (_session != id) return;
      notifyOk(tr('link.linked'));
      widget.onLinked?.call();
      _close();
    } catch (e) {
      _fail(id, e);
    }
  }

  void _switchTab(String next) {
    if (next == _tab) return;
    _session++;
    unawaited(backend.linkCancel().catchError((_) {}));
    setState(() {
      _tab = next;
      _phase = const _Idle();
    });
    if (next == 'mine') unawaited(_startMine());
  }

  void _restart() {
    unawaited(backend.linkCancel().catchError((_) {}));
    if (_tab == 'mine') {
      unawaited(_startMine());
    } else {
      _set(const _Idle());
    }
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    Widget iconTile(String icon, {Color? color}) => Container(
          width: 64,
          height: 64,
          decoration: BoxDecoration(color: t.sunk, borderRadius: BorderRadius.circular(19)),
          child: Center(child: AppIcon(icon, size: 30, color: color ?? t.accentText)),
        );
    Widget centered(List<Widget> children) => Padding(
          padding: const EdgeInsets.only(top: 10, bottom: 2),
          child: Column(crossAxisAlignment: CrossAxisAlignment.center, children: children),
        );

    final p = _phase;
    Widget body;
    if (p is _Confirm) {
      body = centered([
        iconTile('devices'),
        const SizedBox(height: 14),
        Text(context.t('link.confirmTitle', args: {'name': p.name}), textAlign: TextAlign.center, style: ts(17, weight: 700, color: t.ink)),
        if (p.sas != null) ...[
          const SizedBox(height: 10),
          Text(context.t('link.sasLabel'), textAlign: TextAlign.center, style: ts(12.5, color: t.muted)),
          const SizedBox(height: 4),
          Text(p.sas!, textAlign: TextAlign.center, style: ts(26, weight: 800, color: t.ink)),
        ],
        const SizedBox(height: 8),
        Text(context.t(p.sas != null ? 'link.sasConfirmHint' : 'link.confirmBody'), textAlign: TextAlign.center, style: ts(13.5, color: t.muted, height: 1.45)),
        const SizedBox(height: 14),
        Row(children: [
          Expanded(child: Btn(label: context.t('common.cancel'), kind: BtnKind.secondary, height: 44, onPressed: _restart)),
          const SizedBox(width: 8),
          Expanded(child: Btn(label: context.t('link.approve'), height: 44, onPressed: () => _approve(p.link))),
        ]),
      ]);
    } else if (p is _Expired || p is _Failed) {
      body = centered([
        iconTile('alert', color: t.danger),
        const SizedBox(height: 14),
        Text(p is _Expired ? context.t('link.expired') : context.t('errors.title'), textAlign: TextAlign.center, style: ts(17, weight: 700, color: t.ink)),
        const SizedBox(height: 8),
        Text(p is _Failed ? p.message : context.t('link.expiredHint'), textAlign: TextAlign.center, style: ts(13.5, color: t.muted, height: 1.45)),
        const SizedBox(height: 14),
        Btn(
          label: _tab == 'mine' ? context.t('link.newCode') : context.t('link.scanAgain'),
          icon: _tab == 'mine' ? 'qr' : 'scan',
          height: 44,
          onPressed: _restart,
        ),
      ]);
    } else if (p is _Waiting) {
      body = Padding(
        padding: const EdgeInsets.symmetric(vertical: 24),
        child: Column(children: [
          const Spinner(padding: 0),
          const SizedBox(height: 14),
          Text(context.t('link.confirmOther'), textAlign: TextAlign.center, style: ts(16, weight: 700, color: t.ink)),
          if (p.sas != null) ...[
            const SizedBox(height: 10),
            Text(context.t('link.sasLabel'), textAlign: TextAlign.center, style: ts(12.5, color: t.muted)),
            const SizedBox(height: 4),
            Text(p.sas!, textAlign: TextAlign.center, style: ts(26, weight: 800, color: t.ink)),
          ],
          const SizedBox(height: 8),
          Text(context.t(p.sas != null ? 'link.sasWaitHint' : 'link.confirmOtherHint'), textAlign: TextAlign.center, style: ts(13.5, color: t.muted, height: 1.45)),
        ]),
      );
    } else if (_tab == 'mine') {
      body = Column(children: [
        const SizedBox(height: 6),
        if (p is _Code)
          // Tapping the code copies it, for the "or paste the copied code" field of the other device.
          Semantics(
            button: true,
            label: context.t('common.copy'),
            child: GestureDetector(
              onTap: () async {
                // Sensitive: not previewed by the system, cleared after a minute.
                await copySensitive(p.code);
                notifyOk(tr('common.copied'));
              },
              child: QrCodeView(data: p.code),
            ),
          )
        else
          const SizedBox(height: 250, child: Spinner()),
        const SizedBox(height: 14),
        Text(context.t(widget.newDevice ? 'link.showNew' : 'link.showExisting'),
            textAlign: TextAlign.center, style: ts(13, color: t.muted, height: 1.45)),
        const SizedBox(height: 10),
        Row(mainAxisAlignment: MainAxisAlignment.center, children: [
          AppIcon('clock', size: 13, color: t.faint),
          const SizedBox(width: 6),
          Text(context.t('link.validity'), style: ts(12, color: t.faint)),
        ]),
      ]);
    } else {
      body = p is _Busy
          ? const SizedBox(height: 280, child: Spinner())
          : QrScanner(
              hint: context.t(widget.newDevice ? 'link.scanNew' : 'link.scanExisting'),
              pasteLabel: context.t('link.orPaste'),
              placeholder: 'chatapp://link?…',
              submitLabel: context.t('common.continue'),
              onResult: _onScanned,
            );
    }

    return Column(crossAxisAlignment: CrossAxisAlignment.stretch, mainAxisSize: MainAxisSize.min, children: [
      SegTabs(
        tabs: [('mine', 'qr', context.t('qr.myCodeShort')), ('scan', 'scan', context.t('qr.scanShort'))],
        value: _tab,
        onChanged: _switchTab,
      ),
      const SizedBox(height: 16),
      body,
    ]);
  }
}
