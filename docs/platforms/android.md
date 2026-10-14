# Android

## PoC experience
- Send: *Share → Hoppy* from any app, or *Send files* in the app → tap a device.
- Receive: switch **Ready to receive** → foreground notification; each transfer asks with
  Accept/Decline (60 s timeout = decline). Files land in `Download/Hoppy`.
- Overflow menu: theme (Kawaii/Pro), device name, accept automatically (with warning).

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
