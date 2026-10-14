# Android

## PoC experience
- Send: *Share → Hoppy* from any app, or *Send files* in the app → tap a device.
- Share sheet: besides the normal app grid entry (where new apps often rank last), Hoppy
  publishes **sharing shortcuts** so it shows in the sheet's top row: a generic
  *Hoppy · nearby* one (pushed at every app start) and one long-lived shortcut per recent
  destination device (pushed after each successful send, max 4, most recent first).
  Picking a device shortcut opens the send screen and sends to that device as soon as
  discovery finds it; after ~6 s without it the normal list is left to the user. The same
  shortcuts appear on a long-press of the launcher icon and open the file picker.
  Files: `res/xml/shortcuts.xml` (share-target category `app.hoppy.category.SHARE_TARGET`
  → `SendActivity`), `ui/ShareShortcuts.kt` (publishing), `ui/ShareTargets.kt` (pure ids,
  ranking and storage, unit-tested).
- *Send files* on the home screen opens the system document picker and hands the URIs to
  `SendActivity` as `ACTION_SEND` / `ACTION_SEND_MULTIPLE` (EXTRA_STREAM + ClipData + read
  grant), the same shape as an external share.
- Receive: switch **Ready to receive** → foreground notification; each transfer asks with
  Accept/Decline (60 s timeout = decline). Files land in `Download/Hoppy`.
- First launch: theme picker (Purikura, Showa, Manga, Yume, Night), pre-selected from the OS.
- Overflow menu: theme, device name, accept automatically (with warning).

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
