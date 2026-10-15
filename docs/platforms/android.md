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
- Send errors: if the connection cannot be opened (connect timeout / refused / no route,
  `PeerUnreachableException` from `:core`), the send screen says it cannot reach the device
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
  Accept/Decline (60 s timeout = decline). Files land in `Download/Hoppy`.
- End of every accepted receive: a system notification on the *Received files* channel
  (id `done`, default importance/sound), one per transfer: "Received N files from X" (or
  "Transfer from X failed"); tap opens the file (single file with a known type, `ACTION_VIEW`
  + read grant on its MediaStore item) or the Downloads app. Needs the notification
  permission, asked when the receive switch is turned on.
- First launch: theme picker (Purikura, Showa, Manga, Yume, Night), pre-selected from the OS.
- Theme music (`audio/Music.kt`, rules in `docs/design/themes.md` "Music"): idle loop while
  the app is visible, faster loop on the sender during a send, jingle on delivery, silence
  on the receiver while receiving and whenever another app plays audio.
- Overflow menu: theme, device name, music (on by default), accept automatically (with warning).

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
- Newer Android versions are adding local-network permission rules; re-check on each
  targetSdk bump.
- Sending runs inside `SendActivity` in the PoC; leaving the screen keeps the thread alive
  but a service-based sender is planned.
