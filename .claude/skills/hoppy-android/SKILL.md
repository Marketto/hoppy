---
name: hoppy-android
description: Use for the Android app - share target, device picker, receive foreground service, notifications, MediaStore saving, themes in the UI, Gradle setup, and the pure-JVM :core networking module.
---

# Android

Two modules in `apps/android/`:

- `:core` (`core/src/main/kotlin/app/hoppy/core/`) — **pure JVM** protocol, discovery and
  transfer, plus protocol v1 (`Crypto.kt`, `Noise.kt`, `Trust.kt`; BouncyCastle lightweight
  API for X25519/ChaCha20-Poly1305). No `android.*` imports. Unit tests in
  `core/src/test/...` (`CoreTest`, `V1Test` = every `v1` vector); interop with Rust and
  Python via `InteropMain.kt` + `tools/interop.sh` (legs K1–K10).
- `:app` (`app/src/main/java/app/hoppy/`) — thin Android shell:
  - `HoppyApp.kt` — `Prefs` (identity, theme, auto-accept) and `Node` (process-wide
    `DiscoveryNode`, ref-counted, owns the Wi-Fi `MulticastLock`; screens subscribe to `Node`).
  - `service/ReceiveService.kt` — foreground (`dataSync`) receiver; asks via notification
    actions (`DecisionReceiver`), 60 s timeout = decline; `onTimeout` stops it (Android 15).
    `ReceiveService.receiving` is true from offer accepted to connection closed (state
    listeners fire); each finished receive posts its own notification (channel `done`,
    "Received files"; tap opens the single file or Downloads).
  - `service/MediaStoreSink.kt` — saves to `Download/Hoppy`, `IS_PENDING` until hash OK;
    `takeUri(location)` maps a committed file to its MediaStore item.
    Callback (protocol §6): `ReceiveService` sets `Node.callbackHandler` →
    `CallbackReceiver.answer` through the same `serve { }` path as TCP connections;
    `SendActivity` uses `TransferClient.sendToPeer(…, Node.node())` (blocks up to ~23 s,
    holds a `Node` reference meanwhile; its `onFallback` shows `send_trying_callback`). Never
    send with the bare `TransferClient.send`. One Accept/Decline prompt at a time (`deciding`).
  - `audio/Music.kt` — theme music (main thread): visible-activity counter, audio focus,
    idle/transfer/done; `audio/MusicTiming.kt` (pure, + test vs `assets/music/manifest.json`)
    holds bar lengths and fade curves. Hooks: `SendActivity` (`transferStarted/Ended`),
    `ThemePickerActivity` (`preview`), menu item *Music*. Rules in `docs/design/themes.md`.
  - `ui/MainActivity.kt` (home), `ui/SendActivity.kt` (share target + picker), `ui/Common.kt`
    (`ThemedActivity`, `PeerAdapter`, `UriSource`, `Format`).
  - `ui/ShareShortcuts.kt` + `res/xml/shortcuts.xml` — sharing shortcuts (Direct Share):
    generic "Hoppy · nearby" at app start, one per recent device after a delivered send
    (max 4); `SendActivity` reads `Intent.EXTRA_SHORTCUT_ID` and **preselects** that device
    (framed, top of list, "Send to <name> / <IP>" button; 6 s, then the list) — never
    auto-sends. Pure ids/ranking in `ui/ShareTargets.kt` (+ test).
  - `ui/SafeText.kt` (pure, + test) — labels without control/bidi chars, outgoing file
    names, which shared URIs may be read (`content:` of other apps only).
    Home "Send files" forwards picked URIs as `ACTION_SEND(_MULTIPLE)` + ClipData + grant.
  - `update/Updater.kt` — update check (home start, ≤ 1/24 h, skipped on `-dev`) against
    GitHub `releases/latest`, "Updates" channel, home banner, download + SHA-256 check vs
    `SHA256SUMS.txt`, `PackageInstaller` session; `update/InstallReceiver.kt` handles the
    session status (user confirmation). Pure semver/JSON/checksum parsing in
    `update/UpdateLogic.kt` (+ test). Details: `docs/platforms/android.md` "Updates".
  - `ui/ThemePickerActivity.kt` — first-run theme choice; `ui/theme/` — the five themes and
    all animated views (see `hoppy-themes`).
  - v1 security (docs/platforms/android.md "Encryption and device verification"):
    `security/Security.kt` (key + trust store in `noBackupFilesDir`, excluded from backup/transfer; new key ⇒ new device id; `Security.local(ctx)` off the
    main thread, `PendingVerifications`), `ui/VerifyPanel.kt` + `layout/view_verify.xml`
    (4 large emoji + names, *They match* / *Later*), `ui/VerifyActivity.kt` (receiver code /
    changed key). Badges in `PeerAdapter.trusted`.

## Rules

- Minimal UI: mascot, one switch, nearby list, one button. No settings screen; options
  live in the overflow menu.
- Every screen extends `ThemedActivity` and uses theme attributes or `HoppyTheme`, never
  hard-coded colors. See `hoppy-themes`.
- Strings in `values/strings.xml` **and** `values-it/strings.xml` with identical placeholders.
- Network/disk work off the main thread (`thread {}` or the service pool).
- Content URIs with unknown size are copied to cache first (protocol needs sizes).
- Shortcut ids/labels come from peers: validate ids (`ShareTargets.isValidPeerId`) and
  clean names before publishing; keep the share-target category in sync with `ShareTargets.CATEGORY`.
- `SendActivity` and `MainActivity` are exported: never start a transfer from intent
  extras without a user tap; honour a requested device only if it is a recent target
  (`ShareTargets.recentPeer`) or carries the in-process token. Never read `file:` URIs or
  our own providers; never use a provider's `DISPLAY_NAME` as a path.
- Show peer text through `SafeText.label` / `SafeText.offeredFileName`.
- Updater: never rename the release assets (`Hoppy-android.apk`, `SHA256SUMS.txt`) or the
  repo path in `Updater.kt`; never install without a matching SHA-256; only start a
  download from a user tap (banner, or our token-carrying notification).
- Music plays only on the sender and only while a Hoppy screen is visible; never start it
  over other audio, never on the receiver. New screens get it for free (lifecycle callbacks).
- v1: never fall back to plaintext for a peer that advertised `crypto`, whose key or address
  is remembered, or that answered "busy"; KeyChanged always blocks (dialog / notification
  with "Forget device"); pin keys only on a person's Accept; show every plaintext session as
  "non cifrato"; verification is the 4 emoji only (no digits, no QR, no camera permission:
  do not reintroduce them). Never put session data in intents to exported components.

## Build & verify

```
python3 tools/dev.py android-test    # :core JVM tests (needs Gradle); CI also runs :app:testDebugUnitTest
python3 tools/dev.py android-build   # APK (needs Android SDK)
python3 tools/dev.py interop         # Kotlin core vs Rust vs Python
```
No wrapper is committed yet: run `gradle wrapper --gradle-version 8.14.3` once in
`apps/android` (Android Studio offers it). CI builds with Gradle 8.14.3 and uploads the APK.

## Device test

Phone and PC on the same Wi-Fi: `hoppy receive` on PC → app "Send files"; app switch
"Ready to receive" → `hoppy send FILE` on PC. Debug the LAN from a laptop with
`python3 tools/hoppy_ref.py peers`. Guest/corporate Wi-Fi often blocks client-to-client
traffic: that is the network, not the app.
