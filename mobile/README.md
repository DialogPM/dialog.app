# Dialog mobile (Flutter)

The Android / iOS app of Dialog. All the cryptography, storage, networking and sync live in the
shared Rust core (`crates/chat-core`), exposed to Dart through flutter_rust_bridge (`rust/`,
`rust_builder/`, generated bindings in `lib/src/rust/`). The Dart side is UI and state only; it
uses exactly the same commands, JSON requests and events as the desktop app
(`app/src/lib/backend.ts`, `app/src/lib/types.ts`).

## Layout

```
lib/
  main.dart               startup: RustLib.init, DB key, data dir, core start, notifications, push
  app/                    root widget (theme / language / lock / onboarding switch), navigation, globals
  core/models.dart        data types (mirror of types.ts) with fromJson
  core/backend.dart       typed commands, typed event streams, parseError
  state/                  Riverpod store (normalized, updated by core events), actions, preferences
  features/               onboarding, home (tab bar), chats, conversation, info, settings, devices, lock
  services/               local notifications, optional Firebase push, file pick / save / share
  theme/                  design tokens (ThemeExtension), oklch avatar colors, ThemeData
  widgets/                design primitives (icons, buttons, fields, dialogs, QR, emoji picker)
  l10n/                   translations (generated from app/src/locales + tool/mobile_strings.json)
assets/fonts/             Plus Jakarta Sans, Figtree, JetBrains Mono (variable fonts, OFL)
tool/gen_strings.py       regenerates lib/l10n/strings.g.dart
```

The UI implements the mobile variant of `docs/design/ChatApp-Premium.dc.html` (single pane, bottom
tab bar, full-screen info pane) for both visual directions (A "Clasic cald", B "Panouri flotante"),
light and dark, plus a font size setting. Screenshots: `docs/screenshots/mobile/`.

## Build

```sh
flutter pub get
flutter analyze
flutter test
flutter build apk --release                     # Android (needs the release key, see below)
flutter build ios --release                     # iOS (on macOS)
flutter build linux --debug                     # development build, phone-sized window
```

### Default server

The built-in server is chosen at build time (users can still pick a custom server on the welcome
screen):

```sh
flutter build apk --dart-define=CHATAPP_DEFAULT_SERVER=https://chat.example.org
```

Without the define it is `https://chat.dialog.pm`. The release manifest sets
`usesCleartextTraffic="false"`; debug builds add `src/debug/res/xml/network_security_config.xml`,
which allows clear-text HTTP only to `localhost`, `127.0.0.1` and `10.0.2.2` (the emulator's host)
for a development server. The Rust core does its own networking, so a custom `http://` server
(self-hosted on a LAN) still connects; the welcome screen shows a warning when one is selected.

Map tiles default to the public OpenStreetMap servers; production builds should use their own tile
server: `--dart-define=CHATAPP_TILE_URL=https://tiles.example.org/{z}/{x}/{y}.png`.

### Android release signing

Release builds are never signed with the debug key: without a release key `flutter build apk
--release` / `appbundle` stops with an error. Create the key once and keep it (and a backup)
outside the repository; losing it means the app can no longer be updated in place:

```sh
keytool -genkeypair -v -keystore ~/keys/dialog-release.jks -alias dialog \
  -keyalg RSA -keysize 4096 -validity 10000
```

Then create **`android/key.properties`** (gitignored, like `*.jks` / `*.keystore`):

```properties
storeFile=/Users/you/keys/dialog-release.jks
storePassword=...
keyAlias=dialog
keyPassword=...
```

`android/app/build.gradle.kts` reads it and signs the release build. For Google Play, enroll in Play
App Signing and use this key as the upload key. In CI, write `key.properties` and the keystore from
secrets at build time.

## Push notifications (optional: Firebase Cloud Messaging, iOS VoIP for calls)

Push is optional. Without credentials the app works normally and shows local notifications while
it is running; `Firebase.initializeApp()` simply fails and push stays off (no crash). The exact
checklist of what to provide and where is in **[docs/PUSH-SETUP.md](../docs/PUSH-SETUP.md)**:
`android/app/google-services.json`, `ios/Runner/GoogleService-Info.plist` (bundled by the
"Copy Firebase config (optional)" build phase when present, no Xcode change needed), the APNs key
in Firebase, and the server credentials.

How it works in the app:

- After an account is opened the app registers its tokens through the core:
  `set_push_token {provider: "fcm", token}` and, on iOS, `{provider: "apns_voip", token}` (PushKit,
  calls only), again whenever they change. The core adds the account's random push tag, so a push
  tells which of the accounts on this phone it is for and nothing else.
- Pushes carry no content (no sender, no conversation, no text). Android gets data-only messages:
  while the app process runs, the background isolate hands them to the main isolate
  (`services/push.dart`, `state/push_handling.dart`), which connects, decrypts and notifies as usual
  (previews, muted chats, do-not-disturb). When the app is not running no account is open and its
  data cannot be read without the password, so a generic "New message" / "Incoming call" is shown.
  A push for another account on this phone shows "New message in another account"; tapping it
  opens the accounts list.
- iOS shows the alert itself (loc-keys `NEW_MESSAGE` / `INCOMING_CALL` in
  `ios/Runner/{en,ro,it}.lproj/Localizable.strings`). A Notification Service Extension that decrypts
  before display is a later improvement.
- Calls: Android shows a full-screen ringing notification with Answer / Decline (channel `calls`).
  iOS reports VoIP pushes to CallKit (`ios/Runner/AppDelegate.swift`, `VoipCalls`;
  `services/callkit.dart`); answer / decline on the system screen drive `acceptCall` /
  `declineCall` in `state/calls.dart`.

## Platform notes

- Android: minSdk 24, permissions in `android/app/src/main/AndroidManifest.xml` (camera, microphone,
  location, contacts, notifications, biometrics). `MainActivity` is a `FlutterFragmentActivity`
  (required by local_auth). Core library desugaring is enabled (flutter_local_notifications).
  Backups are off (`allowBackup="false"`, `data_extraction_rules.xml` excludes everything).
- Hardening (setting "Block screenshots", on by default): Android sets `FLAG_SECURE` (no screenshots,
  screen recording or recent-apps thumbnail); iOS covers the app in the app switcher and while the
  screen is recorded or mirrored (iOS cannot block screenshots). "Incognito keyboard" (Android, on by
  default) disables keyboard personalized learning. Sensitive copies (device link code) are marked
  sensitive and cleared after 60 s. Temporary plaintext files live in `<temp>/dialog-tmp`, deleted
  after use, at every start and when the app locks. Release builds print no logs (`debugPrint` is
  a no-op).
- iOS data protection: the app data directory uses `CompleteUntilFirstUserAuthentication` (the
  database and media are encrypted by the core with a key derived from the password, and calls /
  push keep working while the screen is locked); the temporary folder uses
  `CompleteUnlessOpen` (unreadable while the device is locked).
- iOS: usage strings (camera, microphone, location, contacts, Face ID, photo library) are in
  `ios/Runner/Info.plist`; bundle id `pm.dialog.chat`.
- The database key is generated on first launch (`newDbKey()`) and kept in the platform keystore
  (flutter_secure_storage: Keychain / Android Keystore). Desktop development builds without a secret
  service (e.g. headless Linux) fall back to a private key file in the app data directory.
- Linux development builds need `libgtk-3-dev` and `libsecret-1-dev`. Camera scanning, biometrics,
  push and voice playback are not available on Linux; the QR dialogs offer "paste the link"
  instead (tapping a device-link QR code copies it).

## Translations

`lib/l10n/strings.g.dart` is generated from the desktop translations (`app/src/locales/*.json`)
plus the mobile-only strings in `tool/mobile_strings.json` (the shared ones always win):

```sh
python3 tool/gen_strings.py
```

The language follows the system (Romanian, English or Italian; English otherwise) unless one is
chosen in Settings → Language.
