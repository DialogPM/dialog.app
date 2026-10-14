import 'package:flutter/material.dart';

import '../../app/globals.dart';
import '../../core/models.dart';
import '../../l10n/i18n.dart';
import '../../services/notifications.dart';
import '../../state/actions.dart';
import '../../state/profiles.dart';
import '../../widgets/dialogs.dart';
import '../../widgets/ui.dart';

/// Asks before logging out of this device: it is unlinked from the account and its local
/// messages are deleted ("Switch account" is the way to keep them, see state/profiles.dart).
Future<void> confirmLogout(BuildContext context) async {
  final ok = await showConfirm(
    context,
    title: context.t('mobile.logoutTitle'),
    body: context.t('mobile.logoutBody'),
    confirmLabel: context.t('settings.devices.logout'),
  );
  if (!ok || !context.mounted) return;
  if (!await confirmLastDevice(context)) return;
  await attempt(logout);
  await Notifications.instance.cancelAll();
}

/// Name of the signed-in account for the warnings: its profile name, else its last digits.
String _accountName(BuildContext context) {
  final name = appData.profile?.name.trim() ?? '';
  if (name.isNotEmpty) return name;
  final number = appData.appState?.account?.number ?? '';
  return number.length >= 4 ? '•••• ${number.substring(number.length - 4)}' : context.t('profiles.account');
}

/// Before logging out: when this phone is the only device of the account, a strong warning
/// (the account cannot receive messages until it logs in again) offers switching account
/// instead. Returns whether to go on with the logout. When the device list cannot be fetched
/// (offline), the logout goes on as already confirmed.
Future<bool> confirmLastDevice(BuildContext context) async {
  List<DeviceInfo>? devices;
  try {
    devices = await backend.listDevices();
  } catch (_) {}
  if (devices == null || !isOnlyDevice(devices)) return true;
  if (!context.mounted) return false;
  final name = _accountName(context);
  // Switching keeps the data on this phone, which needs it locked with the password.
  final canSwitch = appData.appState?.localProtected ?? false;
  final choice = await showAppDialog<String>(
    context,
    title: context.t('profiles.lastDeviceTitle', args: {'name': name}),
    builder: (ctx) => Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
      NoticeBox(tone: 'danger', icon: 'alert', text: ctx.t('profiles.lastDeviceBody', args: {'name': name})),
      const SizedBox(height: 18),
      if (canSwitch) ...[
        Btn(label: ctx.t('profiles.switchInstead'), height: 46, onPressed: () => Navigator.of(ctx).pop('switch')),
        const SizedBox(height: 8),
      ],
      Btn(label: ctx.t('profiles.logoutAnyway'), kind: BtnKind.danger, height: 46, onPressed: () => Navigator.of(ctx).pop('logout')),
      const SizedBox(height: 8),
      Btn(label: ctx.t('common.cancel'), kind: BtnKind.secondary, height: 46, onPressed: () => Navigator.of(ctx).pop()),
    ]),
  );
  if (choice == 'switch') {
    await switchAccount();
    return false;
  }
  return choice == 'logout';
}
