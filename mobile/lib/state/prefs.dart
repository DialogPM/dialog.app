// Preferences kept on this device (like localStorage on desktop): visual direction, theme,
// language, notification options and the per-account app lock (plus its relock delay). Stored as JSON in the app's
// private data directory.

import 'dart:convert';
import 'dart:io';

import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../theme/tokens.dart';

/// "light" | "dark" | "auto"
typedef ThemePref = String;

const double kMinTextScale = 0.85;
const double kMaxTextScale = 1.3;

/// App lock "lock again after the app was in the background" choices, in minutes (0 = only at start).
const List<int> kRelockChoicesMinutes = [0, 1, 5, 60];

class Prefs {
  const Prefs({
    this.direction = Direction.a,
    this.theme = 'auto',
    this.language,
    this.notifPreview = false,
    this.notifications = true,
    this.sound = true,
    this.soundInChat = true,
    this.lockedAccounts = const {},
    this.setupPending = const {},
    this.textScale = 1.0,
    this.recentEmoji = const [],
    this.screenSecurity = true,
    this.incognitoKeyboard = true,
    this.batteryAsked = false,
    this.voiceMask = false,
    this.callDenoise = false,
    this.relockAfterMinutes = 0,
    this.dndUntil = 0,
  });

  final Direction direction;
  final ThemePref theme;

  /// null = follow the system language.
  final String? language;

  /// Show the message text in notifications.
  final bool notifPreview;
  final bool notifications;
  final bool sound;
  final bool soundInChat;

  /// Account ids with the app lock enabled.
  final Set<String> lockedAccounts;

  /// New accounts whose first-run setup (name, username, appearance) is not done yet.
  final Set<String> setupPending;

  /// Font size (text scale factor, 0.85..1.3).
  final double textScale;

  /// Recently used emoji, most recent first.
  final List<String> recentEmoji;

  /// Block screenshots / screen recording and hide the content in the app switcher.
  final bool screenSecurity;

  /// Ask the keyboard not to learn from what is typed (Android).
  final bool incognitoKeyboard;

  /// The battery optimization exemption was offered once (Android).
  final bool batteryAsked;

  /// Anonymize the voice in voice messages (features/conversation/voice.dart).
  final bool voiceMask;

  /// AI noise reduction of the microphone in calls (services/voice_fx.dart).
  final bool callDenoise;

  /// App lock: lock again after the app was in the background this long (minutes);
  /// 0 = only when the app starts (never while it keeps running).
  final int relockAfterMinutes;

  /// Mirror of the account's do not disturb end (ms, 0 = off; see state/dnd.dart), here so the
  /// background push handler can stay silent without starting the core.
  final int dndUntil;

  Prefs copyWith({
    Direction? direction,
    ThemePref? theme,
    String? language,
    bool clearLanguage = false,
    bool? notifPreview,
    bool? notifications,
    bool? sound,
    bool? soundInChat,
    Set<String>? lockedAccounts,
    Set<String>? setupPending,
    double? textScale,
    List<String>? recentEmoji,
    bool? screenSecurity,
    bool? incognitoKeyboard,
    bool? batteryAsked,
    bool? voiceMask,
    bool? callDenoise,
    int? relockAfterMinutes,
    int? dndUntil,
  }) =>
      Prefs(
        direction: direction ?? this.direction,
        theme: theme ?? this.theme,
        language: clearLanguage ? null : (language ?? this.language),
        notifPreview: notifPreview ?? this.notifPreview,
        notifications: notifications ?? this.notifications,
        sound: sound ?? this.sound,
        soundInChat: soundInChat ?? this.soundInChat,
        lockedAccounts: lockedAccounts ?? this.lockedAccounts,
        setupPending: setupPending ?? this.setupPending,
        textScale: textScale ?? this.textScale,
        recentEmoji: recentEmoji ?? this.recentEmoji,
        screenSecurity: screenSecurity ?? this.screenSecurity,
        incognitoKeyboard: incognitoKeyboard ?? this.incognitoKeyboard,
        batteryAsked: batteryAsked ?? this.batteryAsked,
        voiceMask: voiceMask ?? this.voiceMask,
        callDenoise: callDenoise ?? this.callDenoise,
        relockAfterMinutes: relockAfterMinutes ?? this.relockAfterMinutes,
        dndUntil: dndUntil ?? this.dndUntil,
      );

  Map<String, dynamic> toJson() => {
        'direction': direction == Direction.b ? 'B' : 'A',
        'theme': theme,
        'language': language,
        'notifPreview': notifPreview,
        'notifications': notifications,
        'sound': sound,
        'soundInChat': soundInChat,
        'lockedAccounts': lockedAccounts.toList(),
        'setupPending': setupPending.toList(),
        'textScale': textScale,
        'recentEmoji': recentEmoji,
        'screenSecurity': screenSecurity,
        'incognitoKeyboard': incognitoKeyboard,
        'batteryAsked': batteryAsked,
        'voiceMask': voiceMask,
        'callDenoise': callDenoise,
        'relockAfterMinutes': relockAfterMinutes,
        'dndUntil': dndUntil,
      };

  factory Prefs.fromJson(Map<String, dynamic> j) {
    final theme = j['theme'];
    return Prefs(
      direction: j['direction'] == 'B' ? Direction.b : Direction.a,
      theme: theme == 'light' || theme == 'dark' || theme == 'auto' ? theme as String : 'auto',
      language: j['language'] is String ? j['language'] as String : null,
      notifPreview: j['notifPreview'] == true,
      notifications: j['notifications'] != false,
      sound: j['sound'] != false,
      soundInChat: j['soundInChat'] != false,
      lockedAccounts: j['lockedAccounts'] is List ? (j['lockedAccounts'] as List).map((e) => '$e').toSet() : const {},
      setupPending: j['setupPending'] is List ? (j['setupPending'] as List).map((e) => '$e').toSet() : const {},
      textScale: j['textScale'] is num ? (j['textScale'] as num).toDouble().clamp(kMinTextScale, kMaxTextScale) : 1.0,
      recentEmoji: j['recentEmoji'] is List ? (j['recentEmoji'] as List).map((e) => '$e').toList() : const [],
      screenSecurity: j['screenSecurity'] != false,
      incognitoKeyboard: j['incognitoKeyboard'] != false,
      batteryAsked: j['batteryAsked'] == true,
      voiceMask: j['voiceMask'] == true,
      callDenoise: j['callDenoise'] == true,
      relockAfterMinutes: kRelockChoicesMinutes.contains(j['relockAfterMinutes']) ? j['relockAfterMinutes'] as int : 0,
      dndUntil: j['dndUntil'] is int ? j['dndUntil'] as int : 0,
    );
  }
}

/// Where the preferences are stored; set at startup.
String? prefsFilePath;

Prefs loadPrefs() {
  final path = prefsFilePath;
  if (path == null) return const Prefs();
  try {
    final f = File(path);
    if (f.existsSync()) return Prefs.fromJson(Map<String, dynamic>.from(jsonDecode(f.readAsStringSync()) as Map));
  } catch (_) {}
  return const Prefs();
}

class PrefsNotifier extends Notifier<Prefs> {
  @override
  Prefs build() => loadPrefs();

  void update(Prefs Function(Prefs p) fn) {
    state = fn(state);
    final path = prefsFilePath;
    if (path == null) return;
    try {
      File(path).writeAsStringSync(jsonEncode(state.toJson()));
    } catch (_) {}
  }

  void setDirection(Direction d) => update((p) => p.copyWith(direction: d));
  void setTheme(ThemePref t) => update((p) => p.copyWith(theme: t));
  void setLanguage(String? code) => update((p) => code == null ? p.copyWith(clearLanguage: true) : p.copyWith(language: code));

  void setTextScale(double v) => update((p) => p.copyWith(textScale: v.clamp(kMinTextScale, kMaxTextScale)));

  void addRecentEmoji(String e) => update((p) => p.copyWith(recentEmoji: [e, ...p.recentEmoji.where((x) => x != e)].take(24).toList()));

  void setLock(String accountId, bool enabled) => update((p) {
        final s = {...p.lockedAccounts};
        enabled ? s.add(accountId) : s.remove(accountId);
        return p.copyWith(lockedAccounts: s);
      });
}

final prefsProvider = NotifierProvider<PrefsNotifier, Prefs>(PrefsNotifier.new);
