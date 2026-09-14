# AMF Companion

The Remote Control companion app for [AMF](../README.md) — see
[`docs/backlog/remote-control-companion-app-plan.md`](../docs/backlog/remote-control-companion-app-plan.md)
for the feature plan this implements. Android-only for now; iOS is out of
scope until later.

## What's here (Phase 1)

- **Pairing** (`lib/pairing_screen.dart`): enter the server address and
  one-time code shown by the desktop's pairing dialog (`Ctrl+Space Q`), and
  exchange them for a per-device token via `POST /pair/exchange`. QR-code
  scanning is not implemented yet — manual entry only, which is also the
  desktop dialog's own fallback.
- **Status** (`lib/status_screen.dart`): polls `GET /status` every 5s and
  lists every feature's status and attention state, matching the desktop's
  attention (`i`) view. A 401 (token revoked, most likely) clears the stored
  credential and returns to pairing.
- **Credential storage** (`lib/credential_store.dart`): the device token,
  server address, and device id, persisted via `shared_preferences`. One
  credential at a time — pairing again overwrites it.
- **Wire types** (`lib/models.dart`, `lib/api_client.dart`): a direct mirror
  of the JSON shapes in `src/remote_server.rs` in the main repo — that file
  is the source of truth if the two drift.

Not yet built: QR scanning, push notifications (Firebase), prompt-response
(Phase 2), and terminal streaming (Phase 3).

## Toolchain

Flutter and the Android SDK aren't bundled with the main repo's dev setup.
This was set up once, non-sudo, into `~/dev/flutter` and
`~/dev/android-sdk` (see the exports appended to `~/.zshrc`):

```sh
export ANDROID_HOME="$HOME/dev/android-sdk"
export PATH="$HOME/dev/flutter/bin:$ANDROID_HOME/cmdline-tools/latest/bin:$ANDROID_HOME/platform-tools:$PATH"
export JAVA_HOME="$HOME/.sdkman/candidates/java/current"  # any JDK 17+ works
```

`flutter doctor` should show the Flutter and Android toolchain lines green.
Chrome and desktop-Linux targets aren't needed for this project.

## Running

```sh
flutter pub get         # first time / after a pubspec change
flutter analyze         # static analysis
flutter test            # widget tests
flutter build apk --debug
```

To install on a real Android device over USB:

```sh
adb devices              # confirm the device shows up
adb install -r build/app/outputs/flutter-apk/app-debug.apk
```

**Testing pairing against a real AMF server:** the server binds to
`127.0.0.1` only (see Epic 1 in the backlog plan — no LAN/tunnel exposure
yet), so a phone can't reach it directly even on the same Wi-Fi. Over USB,
`adb reverse tcp:<port> tcp:<port>` forwards the phone's own `localhost` to
the desktop's, which lets the app connect to `127.0.0.1:<port>` as if the
server were on the phone. Get `<port>` from the desktop's pairing dialog
(the address it displays, or the `Started` toast when toggling the server
with `Ctrl+Space C`).
