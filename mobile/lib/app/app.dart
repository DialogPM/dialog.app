// Root widget: theme (direction A/B, light/dark/auto), language, and the switch between
// onboarding, lock screen and the main shell.

import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_localizations/flutter_localizations.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../features/calls/call_screen.dart';
import '../features/home/home_shell.dart';
import '../features/lock/data_lock_screen.dart';
import '../features/lock/lock_screen.dart';
import '../features/onboarding/onboarding.dart';
import '../features/onboarding/setup_screen.dart';
import '../l10n/i18n.dart';
import '../services/media.dart';
import '../services/security.dart';
import '../state/actions.dart';
import '../state/background.dart';
import '../state/events.dart';
import '../state/prefs.dart';
import '../state/store.dart';
import '../theme/app_theme.dart';
import '../theme/tokens.dart';
import '../widgets/app_icon.dart';
import '../widgets/ui.dart';
import 'globals.dart';

class ChatApp extends ConsumerStatefulWidget {
  const ChatApp({super.key, this.startError});

  final Object? startError;

  @override
  ConsumerState<ChatApp> createState() => _ChatAppState();
}

class _ChatAppState extends ConsumerState<ChatApp> with WidgetsBindingObserver {
  late final AppLifecycleListener _lifecycle;
  Brightness _platformBrightness = WidgetsBinding.instance.platformDispatcher.platformBrightness;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
    initBackgroundNetworking();
    _lifecycle = AppLifecycleListener(
      onHide: () {
        appInForeground = false;
        onAppHidden();
        setAppHidden(true);
      },
      onShow: () {
        appInForeground = true;
        setAppHidden(false);
        onAppVisible();
      },
      onPause: () {
        appInForeground = false;
        setAppHidden(true);
      },
      onResume: () {
        appInForeground = true;
        setAppHidden(false);
        onAppVisible();
      },
    );
  }

  @override
  void dispose() {
    _lifecycle.dispose();
    WidgetsBinding.instance.removeObserver(this);
    super.dispose();
  }

  @override
  void didChangePlatformBrightness() {
    setState(() => _platformBrightness = WidgetsBinding.instance.platformDispatcher.platformBrightness);
  }

  @override
  void didChangeLocales(List<Locale>? locales) {
    if (ref.read(prefsProvider).language == null) setState(() => currentLanguage = systemLanguage());
  }

  @override
  Widget build(BuildContext context) {
    final p = ref.watch(prefsProvider);
    final dark = p.theme == 'auto' ? _platformBrightness == Brightness.dark : p.theme == 'dark';
    final tokens = ChatTokens.build(p.direction, dark: dark);
    final lang = p.language ?? systemLanguage();
    currentLanguage = lang;
    SystemChrome.setSystemUIOverlayStyle((dark ? SystemUiOverlayStyle.light : SystemUiOverlayStyle.dark).copyWith(
      statusBarColor: Colors.transparent,
      systemNavigationBarColor: tokens.pane,
      systemNavigationBarIconBrightness: dark ? Brightness.light : Brightness.dark,
    ));
    return MaterialApp(
      title: kBrandName,
      debugShowCheckedModeBanner: false,
      navigatorKey: navigatorKey,
      scaffoldMessengerKey: messengerKey,
      theme: buildTheme(tokens),
      locale: Locale(lang),
      supportedLocales: const [Locale('en'), Locale('ro'), Locale('it')],
      localizationsDelegates: GlobalMaterialLocalizations.delegates,
      builder: (context, child) {
        // Font size setting (Appearance), on top of the system text scale.
        final mq = MediaQuery.of(context);
        return MediaQuery(
          data: mq.copyWith(textScaler: TextScaler.linear(mq.textScaler.scale(1) * p.textScale)),
          child: L10n(lang: lang, child: CallOverlay(child: child ?? const SizedBox())),
        );
      },
      home: widget.startError != null ? _StartError(error: widget.startError!) : const _Root(),
    );
  }
}

class _Root extends ConsumerStatefulWidget {
  const _Root();

  @override
  ConsumerState<_Root> createState() => _RootState();
}

class _RootState extends ConsumerState<_Root> {
  Route<void>? _lockRoute;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) => _syncLock(appData.locked));
  }

  /// The lock screen is an opaque route on top of the whole session (nothing of the session is
  /// painted underneath, and back cannot leave it).
  void _syncLock(bool locked) {
    // Temporary plaintext files do not survive a lock (a file picked this minute is kept).
    if (locked) unawaited(wipeAppTemp(olderThan: const Duration(minutes: 1)));
    final nav = navigatorKey.currentState;
    if (nav == null) return;
    if (locked && _lockRoute == null) {
      final route = PageRouteBuilder<void>(
        settings: const RouteSettings(name: 'lock'),
        pageBuilder: (_, _, _) => const LockScreen(),
        transitionDuration: Duration.zero,
        reverseTransitionDuration: const Duration(milliseconds: 150),
      );
      _lockRoute = route;
      nav.push(route);
    } else if (!locked && _lockRoute != null) {
      final route = _lockRoute!;
      _lockRoute = null;
      if (route.isActive) nav.removeRoute(route);
    }
  }

  @override
  Widget build(BuildContext context) {
    ref.listen(storeProvider.select((s) => s.locked), (_, locked) => _syncLock(locked));
    ref.listen(prefsProvider.select((p) => p.screenSecurity), (_, on) => unawaited(applyScreenSecurity(on)));
    final appState = ref.watch(storeProvider.select((s) => s.appState));
    ref.listen(storeProvider.select((s) => s.appState?.status), (prev, next) {
      // Signed out: drop every page of the old session.
      if (prev == 'ready' && next != 'ready') {
        _lockRoute = null;
        navigatorKey.currentState?.popUntil((r) => r.isFirst);
      }
    });
    if (appState == null) return const _Splash();
    if (appState.status == 'locked') return const DataLockScreen(protect: false);
    if (!appState.ready) return Onboarding(defaultServer: appState.defaultServerUrl.isEmpty ? kDefaultServer : appState.defaultServerUrl);
    final pending = appState.pendingMnemonic;
    if (appState.account != null && pending != null && pending.isNotEmpty) {
      // Account created but the recovery phrase was never confirmed: show it again.
      return Onboarding(defaultServer: appState.defaultServerUrl, resumeAccount: appState.account, resumeMnemonic: pending);
    }
    // Linked device or data from an older version: lock it with the password first.
    if (!appState.localProtected) return const DataLockScreen(protect: true);
    // A new account: name, username and appearance first.
    final accountId = appState.account?.accountId;
    if (accountId != null && ref.watch(prefsProvider.select((p) => p.setupPending.contains(accountId)))) {
      return SetupScreen(accountId: accountId);
    }
    return const HomeShell();
  }
}

class _Splash extends StatelessWidget {
  const _Splash();

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    return Scaffold(
      backgroundColor: t.canvas,
      body: Center(
        child: Container(
          width: 64,
          height: 64,
          decoration: BoxDecoration(color: t.accent, borderRadius: BorderRadius.circular(19)),
          child: Center(child: AppIcon('chat', size: 32, color: t.onAccent)),
        ),
      ),
    );
  }
}

class _StartError extends StatelessWidget {
  const _StartError({required this.error});
  final Object error;

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    return Scaffold(
      backgroundColor: t.canvas,
      body: SafeArea(
        child: Padding(
          padding: const EdgeInsets.all(24),
          child: Column(mainAxisAlignment: MainAxisAlignment.center, children: [
            AppIcon('alert', size: 40, color: t.danger),
            const SizedBox(height: 16),
            Text(context.t('mobile.startFailed'), textAlign: TextAlign.center, style: ts(20, weight: 700, color: t.ink)),
            const SizedBox(height: 8),
            Text(context.t('mobile.startFailedHint'), textAlign: TextAlign.center, style: ts(14, color: t.muted)),
            const SizedBox(height: 12),
            Text('$error', textAlign: TextAlign.center, style: ts(12, color: t.faint, mono: true)),
          ]),
        ),
      ),
    );
  }
}
