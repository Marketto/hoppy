# Android

## PoC experience
- Send: *Share → Hoppy* from any app, or *Send files* in the app → tap a device.
- Share sheet: besides the normal app grid entry (where new apps often rank last), Hoppy
  publishes **sharing shortcuts** so it shows in the sheet's top row: a generic
  *Hoppy · nearby* one (pushed at every app start) and one long-lived shortcut per recent
  destination device (pushed after each successful send, max 4, most recent first).
  Picking a device shortcut opens the send screen with that device **preselected, never
  auto-sent**: once discovery finds it, it is framed at the top of the list and a primary
  *Send to <name> / <IP>* button appears; the user must tap. After ~6 s without it the
  normal list is left to the user. `SendActivity` and `MainActivity`'s
  `app.hoppy.action.SEND_FILES` are exported, so a requested device is honoured only if it
  is a recent target (`Prefs.recentTargets`) or comes from our own home screen (per-process
  token). The same shortcuts appear on a long-press of the launcher icon and open the file
  picker.
  Files: `res/xml/shortcuts.xml` (share-target category `app.hoppy.category.SHARE_TARGET`
  → `SendActivity`), `ui/ShareShortcuts.kt` (publishing), `ui/ShareTargets.kt` (pure ids,
  ranking and storage, unit-tested).
- Callback (reverse connect, protocol §6, ADR 0006): `SendActivity` sends with
  `TransferClient.sendToPeer(peer, Security.local(ctx), sources, Node.node(), listener)` — direct TCP for 3 s, then it
  asks the receiver over UDP to connect back (up to ~20 s more). The send holds a `Node`
  reference so the discovery socket (bound to UDP 47470, where the PC's announces arrive)
  stays open even if the screen stops. When the direct attempt fails and callback mode
  starts, `:core` calls `SendListener.onFallback` (Rust `Progress::Fallback`) and the status reads
  "<name> isn't answering right away, trying another way…"; a direct send never shows it.
  Receiving: `ReceiveService` sets `Node.callbackHandler` (re-applied after a `Node`
  restart) to `CallbackReceiver.answer` with the same sink and per-connection handler as
  the TCP accept loop, so the accept prompt, `receiving` flag (music silence) and
  end-of-receive notification apply to connected-back transfers too. The node only
  answers while `receivePort != 0`. Callback transfers run on `DiscoveryNode` threads, not
  on the service pool, so the service tracks every transfer socket (direct and callback,
  via `answer(..., onSocket)`) and closes them in `onDestroy`; an open Accept prompt is
  declined, partial files are aborted.
- Send errors: if neither the direct connection nor the callback works (connect timeout /
  refused / no route, `PeerUnreachableException` from `:core`), the send screen says it cannot reach the device
  and suggests allowing Hoppy in the Windows firewall (*Private networks*) and checking both
  are on the same Wi-Fi; the raw exception goes to logcat only (tag `HoppySend`). A refusal
  shows "<name> said no"; other failures show "Could not send: <reason>".
- *Send files* on the home screen opens the system document picker and hands the URIs to
  `SendActivity` as `ACTION_SEND` / `ACTION_SEND_MULTIPLE` (EXTRA_STREAM + ClipData + read
  grant), the same shape as an external share.
- Shared URIs: only other apps' `content:` URIs are read; `file:` URIs and our own
  providers (`app.hoppy*`, also `<user>@app.hoppy…`) are refused (`ui/SafeText.kt`).
  Provider `DISPLAY_NAME`s go through the §4 sanitizer; unknown-size streams are copied to
  a fresh `cache/outgoing/<random>/` directory (canonical path checked), deleted when the
  send screen finishes, stale ones pruned after 24 h.
- Peer-supplied text shown in the UI/notifications (device names, offered file names) is
  stripped of control and Unicode format characters (bidi overrides); offered file names
  are shown as the §4 sanitizer would save them.
- Receive: switch **Ready to receive** → foreground notification; each transfer asks with
  Accept/Decline (60 s timeout = decline). One prompt at a time, as on Windows: an offer
  arriving while another awaits an answer is declined (auto-accept skips the prompt). Files land in `Download/Hoppy`.
- End of every accepted receive: a system notification on the *Received files* channel
  (id `done`, default importance/sound), one per transfer: "Received N files from X" (or
  "Transfer from X failed"); tap opens the file (single file with a known type, `ACTION_VIEW`
  + read grant on its MediaStore item) or the Downloads app. Needs the notification
  permission, asked when the receive switch is turned on.
- First launch: theme picker (Purikura, Showa, Manga, Yume, Night), pre-selected from the OS.
- Theme music (`audio/Music.kt`, rules in `docs/design/themes.md` "Music"): idle loop while
  the app is visible, faster loop on the sender during a send, jingle on delivery, silence
  on the receiver while receiving and whenever another app plays audio.
- Overflow menu: theme, device name, *Show my QR* / *Scan a QR* (device verification),
  music (on by default), accept automatically (with warning).

## Encryption and device verification (protocol v1, ADR 0010)
- `:core` speaks v1 (spec §7) itself, pure JVM: `Crypto.kt` (X25519 + ChaCha20-Poly1305 from
  BouncyCastle's lightweight API, SHA-256/HMAC from the JDK; fingerprints, SAS + emoji table,
  QR payload, `DeviceKey`), `Noise.kt` (Noise XX with the commit/reveal payloads, `u16`
  framing, `NoiseInputStream`/`NoiseOutputStream`, `Text.display`), `Trust.kt` (trust store,
  `HandshakeGuard`), `Transfer.kt` (`Local`, version choice, sender/receiver sessions,
  `confirmQr`, `QrVerification.scan`). Tests: `V1Test` (every `v1` vector incl. `negative`,
  RFC KATs, sessions over TCP) + `CoreTest`; interop legs K1–K10 in `tools/interop.sh`.
- Identity: `security/Security.kt` — `filesDir/identity.key` (created on first run, never
  overwritten if a newer app wrote it) and `filesDir/trust.json`, one `HandshakeGuard` per
  process; `Security.local(ctx)` (first call reads the files: off the main thread, `warm` at
  app start). The device id stays in `Prefs`.
- Device rows: `Windows · 🔒` (encrypted), `· 🔒 ✓` (verified), `· ⚠ not encrypted` (old v0
  app); TalkBack reads "encrypted / verified / not encrypted" (`PeerAdapter.trusted`, loaded
  off the main thread by `refreshTrustBadges`). A remembered id always gets 🔒.
- Sending (`SendActivity`): the session line under the status (🔒 / ✓ / ⚠ not encrypted);
  for a New/Known device `view_verify.xml` (`ui/VerifyPanel.kt`) covers the list while the
  receiver decides: 4 emoji (system emoji font, names as contentDescription and caption),
  "123" ↔ "Emoji" toggle for the 8 digits, *They match* (marks verified), *Verify with QR*,
  *Show my QR*, *Later*. After delivery the screen stays until the code is answered.
  `KeyChangedException` → blocking dialog with *Forget device* (nothing was sent);
  `ReceiverBusyException` → "busy, try again in a minute" (never a plaintext retry);
  `UnencryptedException` → "did not answer with encryption, nothing sent".
- Receiving (`ReceiveService`): the offer notification has a session line (🔒 New/Not
  verified + code, ✓ verified, ⚠ not encrypted) and, for unverified encrypted senders, a
  *Verify* action → `VerifyActivity` (not exported; the session stays in memory,
  `PendingVerifications`) with the same panel; verifying refreshes the notification to ✓.
  Accept/Decline and the 60 s default decline are unchanged. The sender's key is pinned only
  when a person accepts (not with *Accept automatically*). Refused senders (changed key,
  plaintext from a device that encrypts) and code-guessing warnings go to the *Security
  warnings* channel (`security`), rate-limited by the core; a changed-key notification opens
  `VerifyActivity` with *Forget device*.
- QR (`ui/QrActivity.kt`, ZXing Android Embedded): *Scan a QR* / *Verify with QR* ask for the
  camera on use (denied → compare the code); `QrVerification.scan` checks the fingerprint
  against the live session or the remembered key, marks it verified, then sends the one-time
  code to that device over a session that must present exactly the scanned key (a device
  never seen before is verified only that way). *Show my QR* renders `Local.beginQr()`
  (dark on white), switches receiving on while shown (the scanner connects to us), shows ✓
  and closes when a scanner proves it, expires after 10 min.

## Updates (in-app, from GitHub Releases)
The APK is distributed from GitHub Releases, not the Play Store; every release is signed
with the same key (docs/release.md), so a new APK installs over the old one in place.
Code: `update/Updater.kt` (Android side), `update/UpdateLogic.kt` (pure: semver, release
JSON, `SHA256SUMS.txt` parsing; unit-tested), `update/InstallReceiver.kt`.
- **Check**: when the home screen starts, at most once per 24 h (`Prefs.updateCheckedAt`),
  a background thread GETs `https://api.github.com/repos/Marketto/hoppy/releases/latest`
  (`HttpURLConnection`, `User-Agent: Hoppy/<versionName>`, 15 s/30 s timeouts). Silent on any
  error; the timestamp is stored only after a complete response, so a failed check
  retries on the next start. Builds whose `versionName` contains `-dev` (local/CI builds)
  never check. A release counts only if its `tag_name` is exactly `[vV]?X.Y.Z` (it is
  used in the download URL) and it carries both `Hoppy-android.apk` and `SHA256SUMS.txt`;
  version and tag are stored together. Pre-releases are never offered (as on Windows).
- **Newer version**: one notification per version on the *Updates* channel (`updates`,
  default importance): "Hoppy <ver> is available — tap to update"; plus a small banner on
  the home screen (peer-row card + `colorOnSurface`, so it inherits the contrast-checked
  pair) with an *Update* button. The banner uses the last seen version and disappears
  once the installed version catches up.
- **Update**: the notification tap (it carries a secret token from private prefs, since
  `MainActivity` is exported) or the banner button. If Hoppy may not install unknown apps
  (`canRequestPackageInstalls()` false) the system page
  `Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES` for our package opens first and the update
  continues when the user comes back with it allowed. Then: `SHA256SUMS.txt` and
  `Hoppy-android.apk` from `releases/download/<tag>/` of the stored tag, so both come
  from the same release (redirects stay on https; no stored tag → error and a new check) into
  `cacheDir/updates/`, progress in a silent notification, SHA-256 compared with the listed
  hash, package name checked, then a `PackageInstaller` session (`createSession` /
  `openWrite` / `commit`) and the file is deleted. `InstallReceiver` gets the status:
  `STATUS_PENDING_USER_ACTION` → starts the system confirmation (`EXTRA_INTENT`) if a
  Hoppy screen is visible, else posts "tap to install"; errors and hash mismatches → an
  error notification and the download is deleted. The system rejects an APK signed with a
  different key.
- Permission: `REQUEST_INSTALL_PACKAGES`. Notifications need the notification permission
  (asked when receiving is turned on); without it the banner still works.

## Platform constraints
- minSdk 29 (MediaStore Downloads without storage permission), targetSdk 35.
- Receiver is a `dataSync` foreground service: Android 15 limits it to ~6 h/day; the
  service stops itself in `onTimeout`. A long-lived receiver will need a different design
  (e.g. on-demand wake via BLE or push) — tracked in the roadmap.
- Discovery needs `CHANGE_WIFI_MULTICAST_STATE` and a held `MulticastLock` (`Node`).
- Notifications permission (Android 13+) is required to ask before receiving; without it
  incoming transfers are declined unless auto-accept is on.
- Camera permission only for scanning verification QR codes, asked on use; the camera is
  not a required feature (`android.hardware.camera.any` optional).
- APK size: BouncyCastle (`bcprov-jdk18on`) is packaged unshrunk while R8 is off.
- Newer Android versions are adding local-network permission rules; re-check on each
  targetSdk bump.
- Sending runs inside `SendActivity` in the PoC; leaving the screen keeps the thread alive
  but a service-based sender is planned.
