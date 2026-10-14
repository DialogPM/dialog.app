# Dialog

Dialog is an end-to-end encrypted messenger. This repository holds the mobile app (Android, iOS)
and the Rust core it is built on:

- `mobile/` — the Flutter app; `mobile/rust` is the bridge to the core (flutter_rust_bridge)
- `crates/chat-core` — the client core: Signal protocol sessions (libsignal), sealed sender,
  private groups (zkgroup), key transparency, the encrypted local database (SQLCipher)
- `crates/voice-fx` — on-device voice effects and noise suppression
- `docs/` — architecture, server API and protocol notes

The desktop app is at [DialogPM/desktop](https://github.com/DialogPM/desktop).

## Build

Requirements: Flutter (stable), Rust (stable) with the Android/iOS targets, and the Android SDK
+ NDK or Xcode. See [mobile/README.md](mobile/README.md) for details, release signing and push setup.

```bash
cd mobile
flutter pub get
flutter run
```

The app connects to `https://chat.dialog.pm` by default. To build it for another server:

```bash
flutter build apk --dart-define=CHATAPP_DEFAULT_SERVER=https://chat.example.org
```

Push notifications need your own Firebase configuration (`android/app/google-services.json`,
`ios/Runner/GoogleService-Info.plist`); without it the app works and notifies while running.

## License

[GNU Affero General Public License v3.0](LICENSE).
