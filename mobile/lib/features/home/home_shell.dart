// Main screen of the phone layout: the chat list or the settings menu, with the bottom tab bar
// (Chats / QR code / Settings, pill-highlighted icons) and the connection banner.

import 'dart:async';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../../app/globals.dart';
import '../../services/battery.dart';

import '../chats/qr_sheet.dart';
import '../lock/data_lock_screen.dart';
import '../../l10n/i18n.dart';
import '../../state/store.dart';
import '../../theme/app_theme.dart';
import '../../widgets/app_icon.dart';
import '../../widgets/dialogs.dart';
import '../../widgets/ui.dart';
import '../chats/chat_list.dart';
import '../settings/account_security.dart';
import '../settings/key_transparency.dart';
import '../settings/settings_menu.dart';

class HomeShell extends ConsumerStatefulWidget {
  const HomeShell({super.key});

  @override
  ConsumerState<HomeShell> createState() => _HomeShellState();
}

class _HomeShellState extends ConsumerState<HomeShell> {
  int _tab = 0;
  bool _qrOpen = false;

  Future<void> _openQr() async {
    setState(() => _qrOpen = true);
    await showQrSheet(context);
    if (mounted) setState(() => _qrOpen = false);
  }

  bool _askedLocalPassword = false;
  bool _askedWeakPassword = false;
  bool _checkedBattery = false;

  /// Android: offer the battery optimization exemption once, so calls are not cut when the
  /// screen locks and messages arrive on time.
  Future<void> _offerBatteryExemption() async {
    if (!Platform.isAndroid || prefs.batteryAsked || await BatteryOptimization.isExempt()) return;
    if (!mounted) return;
    prefsNotifier.update((x) => x.copyWith(batteryAsked: true));
    final ok = await showConfirm(
      context,
      title: context.t('mobile.battery.title'),
      body: context.t('mobile.battery.body'),
      confirmLabel: context.t('mobile.battery.allow'),
      danger: false,
    );
    if (ok) await BatteryOptimization.request();
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    // Password changed on another device: offer to move the local lock to it (once per run).
    final outdated = ref.watch(storeProvider.select((s) => s.details?.localPasswordOutdated ?? false));
    if (outdated && !_askedLocalPassword) {
      _askedLocalPassword = true;
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (mounted) promptLocalPasswordUpdate(context);
      });
    }
    // Password below the current rules (older account): suggest changing it, once per run.
    final weak = ref.watch(storeProvider.select((s) => s.details?.passwordWeak ?? false));
    if (weak && !outdated && !_askedWeakPassword) {
      _askedWeakPassword = true;
      WidgetsBinding.instance.addPostFrameCallback((_) async {
        if (!mounted) return;
        final change = await showConfirm(
          context,
          title: context.t('password.weakTitle'),
          body: context.t('password.weakBody'),
          confirmLabel: context.t('password.weakAction'),
          danger: false,
        );
        if (change && mounted) setState(() => _tab = 1);
      });
    }
    if (!_checkedBattery && !outdated && !weak) {
      _checkedBattery = true;
      WidgetsBinding.instance.addPostFrameCallback((_) => _offerBatteryExemption());
    }
    return PopScope(
      canPop: _tab == 0,
      onPopInvokedWithResult: (didPop, _) {
        if (!didPop) setState(() => _tab = 0);
      },
      child: Scaffold(
        backgroundColor: t.pane,
        body: Column(
          children: [
            const ConnectionBanner(),
            const KtAlertBanner(),
            const NewDeviceBanner(),
            const LoginRequestWatcher(),
            Expanded(
              child: SafeArea(
                bottom: false,
                top: false,
                child: IndexedStack(index: _tab, children: const [ChatsPane(), SettingsMenu()]),
              ),
            ),
            _TabBar(
              index: _qrOpen ? 2 : _tab,
              onChats: () => setState(() => _tab = 0),
              onQr: _openQr,
              onSettings: () => setState(() => _tab = 1),
            ),
          ],
        ),
      ),
    );
  }
}

/// Offline / connecting / history-restore banner (also covers the status bar area).
///
/// Connecting is normal when the app opens or comes back from the background (the connection
/// is closed there, state/background.dart): the banner shows only when the app has not been
/// online for [_connectionGrace], i.e. when connecting really is slow or failing.
class ConnectionBanner extends ConsumerStatefulWidget {
  const ConnectionBanner({super.key});

  @override
  ConsumerState<ConnectionBanner> createState() => _ConnectionBannerState();
}

const _connectionGrace = Duration(seconds: 5);

class _ConnectionBannerState extends ConsumerState<ConnectionBanner> {
  Timer? _grace;
  bool _showConnection = false;

  @override
  void dispose() {
    _grace?.cancel();
    super.dispose();
  }

  /// Starts the grace period when the app stops being online; hides the banner when back online.
  void _track(String connection) {
    if (connection == 'online') {
      _grace?.cancel();
      _grace = null;
      // Called from build: this build already uses the new value.
      _showConnection = false;
    } else if (_grace == null && !_showConnection) {
      _grace = Timer(_connectionGrace, () {
        _grace = null;
        if (mounted && ref.read(storeProvider).connection != 'online') setState(() => _showConnection = true);
      });
    }
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    final connection = ref.watch(storeProvider.select((s) => s.connection));
    _track(connection);
    final sync = ref.watch(storeProvider.select((s) => s.historySync));
    final top = MediaQuery.of(context).padding.top;
    Widget banner(Color bg, Color fg, Widget lead, String text, {required bool first}) => Container(
      color: bg,
      padding: EdgeInsets.fromLTRB(12, 7 + (first ? top : 0), 12, 7),
      child: Row(
        mainAxisAlignment: MainAxisAlignment.center,
        children: [
          lead,
          const SizedBox(width: 8),
          Flexible(
            child: Text(
              text,
              textAlign: TextAlign.center,
              style: ts(13, weight: 600, color: fg),
            ),
          ),
        ],
      ),
    );
    final children = <Widget>[];
    if (connection != 'online' && _showConnection) {
      final connecting = connection == 'connecting';
      final bg = connecting ? t.sysBg : Color.alphaBlend(t.danger.withValues(alpha: .14), t.pane);
      final fg = connecting ? t.sysFg : t.danger;
      children.add(
        banner(
          bg,
          fg,
          connecting
              ? SizedBox.square(dimension: 14, child: CircularProgressIndicator(strokeWidth: 2, color: fg))
              : AppIcon('wifioff', size: 16, color: fg),
          context.t(connecting ? 'connection.connecting' : 'connection.offline'),
          first: true,
        ),
      );
    }
    if (sync != null) {
      children.add(
        banner(
          t.accentSoft,
          t.accentText,
          sync.done
              ? AppIcon('download', size: 16, color: t.accentText)
              : SizedBox.square(dimension: 14, child: CircularProgressIndicator(strokeWidth: 2, color: t.accentText)),
          sync.done ? context.t('sync.done') : context.t('sync.progress', count: sync.records),
          first: children.isEmpty,
        ),
      );
    }
    if (children.isEmpty) {
      return SizedBox(height: top, child: ColoredBox(color: t.pane));
    }
    return Column(mainAxisSize: MainAxisSize.min, children: children);
  }
}

class _TabBar extends ConsumerWidget {
  const _TabBar({required this.index, required this.onChats, required this.onQr, required this.onSettings});

  final int index;
  final VoidCallback onChats;
  final VoidCallback onQr;
  final VoidCallback onSettings;

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    final t = context.tk;
    final unread = ref.watch(storeProvider.select((s) => s.unreadChats));
    final bottom = MediaQuery.of(context).padding.bottom;
    Widget tab(String label, String icon, bool active, VoidCallback onTap, [int badge = 0]) => Expanded(
      child: Semantics(
        button: true,
        selected: active,
        label: label,
        child: GestureDetector(
          behavior: HitTestBehavior.opaque,
          onTap: onTap,
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              Stack(
                clipBehavior: Clip.none,
                children: [
                  AnimatedContainer(
                    duration: const Duration(milliseconds: 150),
                    width: 58,
                    height: 30,
                    decoration: BoxDecoration(color: active ? t.accentSoft : Colors.transparent, borderRadius: BorderRadius.circular(999)),
                    child: Center(child: AppIcon(icon, size: 21, color: active ? t.accentText : t.muted)),
                  ),
                  if (badge > 0)
                    Positioned(
                      top: -3,
                      right: 8,
                      child: Container(
                        constraints: const BoxConstraints(minWidth: 17),
                        height: 17,
                        padding: const EdgeInsets.symmetric(horizontal: 4),
                        decoration: BoxDecoration(
                          color: t.accent,
                          borderRadius: BorderRadius.circular(999),
                          border: Border.all(color: t.pane, width: 2),
                        ),
                        alignment: Alignment.center,
                        child: Text(badge > 99 ? '99+' : '$badge', style: ts(10, weight: 700, color: t.onAccent, height: 1)),
                      ),
                    ),
                ],
              ),
              const SizedBox(height: 4),
              Text(label, style: ts(11.5, weight: 650, color: active ? t.accentText : t.muted)),
            ],
          ),
        ),
      ),
    );
    return Container(
      padding: EdgeInsets.fromLTRB(12, 8, 12, bottom > 0 ? bottom + 8 : 14),
      decoration: BoxDecoration(
        color: t.pane,
        border: Border(top: BorderSide(color: t.line)),
      ),
      child: Row(
        children: [
          tab(context.t('chats.title'), 'chat', index == 0, onChats, unread),
          tab(context.t('qr.title'), 'qr', index == 2, onQr),
          tab(context.t('settings.title'), 'gear', index == 1, onSettings),
        ],
      ),
    );
  }
}
