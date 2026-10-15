# macOS and iPhone/iPad

## macOS

The same experience as on Windows and Ubuntu (ADR 0008): a menu-bar icon that receives,
"Send with Hoppy" from the Finder, a device menu at the pointer, notifications, the delivery
jingle and an update notice.
Target: **macOS 12 Monterey and later** — the newest macOS for 2015 MacBook Pro/Air, so it
covers Intel Macs from 2015 and every Apple silicon Mac (M1 and later). One universal
binary (x86_64 + arm64, `lipo`), built with `MACOSX_DEPLOYMENT_TARGET=12.0`.

### Install and first launch (not notarized)

Hoppy has no Apple Developer ID yet: `Hoppy.app` is signed **ad hoc** (`codesign -s -`) and
not notarized, so Gatekeeper blocks the first launch once. The .dmg contains `Hoppy.app`, a
link to Applications and `Primo avvio - First launch.txt` with these steps:

1. Open `Hoppy-macos-universal.dmg` and drag **Hoppy** onto **Applications**.
2. Double-click Hoppy in Applications. macOS says it cannot verify the developer: choose
   **Done** (never "Move to Trash").
3. **System Settings → Privacy & Security**, scroll to "Hoppy was blocked to protect your
   Mac", **Open Anyway** («Apri comunque»), confirm with the password. On macOS 12–14,
   right-click Hoppy → **Open** → **Open** also works; **on macOS 15 Sequoia and later the
   right-click route no longer bypasses Gatekeeper**: only *Open Anyway* in Settings does.
4. Answer the prompts: notifications (**Allow**), local network on macOS 15+ (**Allow**;
   without it no device is found), and the first time files are saved, access to the
   Downloads folder (**OK**).

Every new version is a new ad-hoc signature: steps 2–3 and the privacy prompts may come back
after an update. Running Hoppy straight from the .dmg or from Downloads works, but macOS
then runs a temporary copy (App Translocation), so "Open at login" stays off until Hoppy is
in Applications.

### How it works

| Piece | Where | Notes |
| --- | --- | --- |
| Shared tray/send logic, IT/EN strings | `crates/hoppy-cli/src/gui.rs` | Same as Windows/Linux. `NATIVE_POPUPS = true`: the menu is built at each click. Extra menu entry **Open at login** (`platform::login_item`). |
| AppKit backend (`platform::ui`) | `crates/hoppy-cli/src/macos/ui.rs` | objc2 bindings, no Swift/Xcode project. Main thread runs the event loop (`Tray::next_event` pumps `NSApplication` events); workers wake it with an application-defined `NSEvent` posted from the main dispatch queue; dialogs from workers run on the main thread through `dispatch2::run_on_main`. |
| Pure helpers (tested on Linux) | `crates/hoppy-cli/src/macos/mod.rs` | Bundle path, App-Translocation check, LaunchAgent plist, osascript argv, language, status title; a test keeps `apps/macos/Info.plist` in sync with the code. |
| Login item | `crates/hoppy-cli/src/macos/login.rs` | macOS 13+: `SMAppService.mainApp` (Login Items lists Hoppy); if the system refuses it, or on macOS 12: `~/Library/LaunchAgents/app.hoppy.desktop.login.plist` (`open -a Hoppy.app`, carries a marker, never touches a file without it). |
| Bundle, Finder service, document types | `apps/macos/Info.plist`, `apps/macos/en.lproj/`, `apps/macos/it.lproj/` | `LSUIElement` (no Dock icon), `LSMinimumSystemVersion` 12.0, `NSServices` "Send with Hoppy" («Invia con Hoppy»), `CFBundleDocumentTypes` (`public.data`, rank Alternate), privacy prompt texts. |
| Packaging | `apps/macos/build-app.sh` | Builds both architectures, `lipo`, checks `minos` ≤ 12.0 per architecture, assembles and signs `Hoppy.app`, makes the .dmg (`hdiutil`, retried). |
| Icons (generated) | `assets/brand/macos/` (`python3 tools/gen_icons.py --macos`) | `Hoppy.icns` + `hoppy-1024.png` (824 px body on the 1024 grid) and `menubar-template.png` (36 px = 18 pt @2x, black + alpha template image, tinted by macOS). |

- **Receive**: `Hoppy.app` (= `Contents/MacOS/hoppyw`) is a menu-bar icon (the bunny,
  template image). Each offer asks with an `NSAlert`: **Rifiuta/Decline** is the first button
  (Return), **Accetta/Accept** the second; no answer within 110 s = declined; a second offer
  while one is asked is declined (`gui::GuiReceiver`). Files go to `~/Downloads/Hoppy`. A
  notification "Ricevuti N file da …"; clicking it opens the folder.
- **Send**: Finder → select files → right-click → **Services → Invia con Hoppy** (Finder
  menu → Services works too), or drop files on the Hoppy icon (Applications, Launchpad
  shortcut, Finder sidebar), or *Open With → Hoppy*, or the menu-bar icon → *Invia file…*.
  The tray starts `hoppyw send FILE…` (a second status item shows "Cerco dispositivi…",
  then the progress as its title); the device menu pops up at the mouse pointer; silent
  until the done jingle (`NSSound`, the embedded `desktop_done.wav`) unless muted
  (*♪ Suono a fine invio*). Clicking the sender's icon offers *Annulla invio*.
- **Notifications**: `UNUserNotificationCenter` when running from the bundle (needs
  permission once). If the API is unavailable — a bare binary (`cargo run`) or the system
  refusing the ad-hoc signature (error at authorization) — Hoppy falls back to
  `osascript display notification` (texts passed as script arguments after a fixed
  sentinel, never inside the script; shown under Script Editor's name; a click does
  nothing useful, so *Apri cartella ricevuti* in the menu is the way to the files). If the
  user turned Hoppy's notifications off, nothing is shown (no osascript workaround).
- **Single instance**: an exclusive lock on `tray.lock` next to `device.json`. A second
  launch asks the running one to show itself through Launch Services (`open -b
  app.hoppy.desktop` → `applicationShouldHandleReopen:` → "Hoppy is already running").
- **Updates**: like Linux, notify + link to the portal (*Download Hoppy X…*), never
  download or install: an unsigned app replaced in place would trip Gatekeeper anyway. A
  release counts only once `Hoppy-macos-universal.dmg` and `SHA256SUMS.txt` are attached.
- **Log**: `~/Library/Logs/Hoppy/hoppy.log` (Console.app shows it; menu *About &
  diagnostics…* opens it in TextEdit; `hoppy log` from Terminal).
- **CLI**: `Hoppy.app/Contents/MacOS/hoppy`. Optional: `sudo ln -s
  /Applications/Hoppy.app/Contents/MacOS/hoppy /usr/local/bin/hoppy`. `hoppy install` /
  `hoppy uninstall` turn "Open at login" on/off (uninstall also stops the tray from turning
  it back on).
- **Firewall**: the macOS application firewall is **off by default**. If it is on, macOS
  asks "Accept incoming network connections?" the first time Hoppy receives — and, for an
  ad-hoc signed app, possibly again after each update. Deny = other devices cannot send to
  this Mac directly; the callback fallback (spec §6, ADR 0006) still lets them deliver when
  only inbound connections are blocked, since the Mac then connects out. Fix: System
  Settings → Network → Firewall → Options → Hoppy: Allow incoming connections.
- **Local Network privacy (macOS 15+)**: the prompt text comes from
  `NSLocalNetworkUsageDescription`; if denied, discovery finds nothing. Fix: System
  Settings → Privacy & Security → Local Network → Hoppy on.

### Upgrade path: Developer ID + notarization

Only CI secrets are missing (docs/release.md "Signing"): with `APPLE_CERT_P12_BASE64` +
`APPLE_CERT_PASSWORD`, `release.yml` imports the certificate into a temporary keychain and
`build-app.sh` signs with the hardened runtime and a timestamp; with `APPLE_ID`,
`APPLE_TEAM_ID`, `APPLE_APP_PASSWORD` it also notarizes and staples the .dmg. Then
Gatekeeper opens Hoppy with a single confirmation, privacy grants survive updates, and an
in-app updater (like Windows) becomes possible.

### Limits (product facts)

- A top-level Finder context-menu entry needs a Finder Sync / Action extension (an `.appex`
  in a signed bundle, built with Xcode): roadmap. Today the entry is under *Services* (or
  *Quick Actions* when Finder groups them); folders are not offered (files only, `public.data`).
- The Share menu ("Condividi") needs a Share extension (`.appex`): roadmap, with signing.
- Not notarized: see above. Hoppy never asks for administrator rights.

### Manual test plan (real Macs)

Run on an **Intel Mac from 2015 on macOS 12** and an **Apple silicon Mac on macOS 15**
(14 if available), each with an Android phone and a Windows/Linux PC on the same Wi-Fi.
Check `~/Library/Logs/Hoppy/hoppy.log` after each block.

1. **Install**: .dmg opens with Hoppy + Applications + the text file; drag to Applications;
   first launch blocked; *Open Anyway* in Privacy & Security works (macOS 15: right-click →
   Open does **not**; macOS 12: it does); `lipo -archs` lists `x86_64 arm64`.
2. **Prompts**: notifications permission asked once; Local Network (15+) asked when the
   tray starts; Downloads access asked at the first receive; sending from Desktop/Documents
   may ask for that folder once; macOS 13+ shows "Login item added"; the texts are Italian
   on an Italian system.
3. **Menu bar**: bunny icon tinted correctly in light and dark menu bars and on a
   non-Retina display (2015 MacBook Air); left and right click open the menu at once under
   the icon; entries: header, Invia file…, Apri cartella ricevuti, ♪ Suono a fine invio,
   Apri all'accesso, Informazioni e diagnostica…, Esci; no Dock icon, no app menu.
4. **Receive**: phone → Mac: alert in front of other apps (also over a full-screen app and on
   another Space), Return = Rifiuta, Accetta saves to
   `~/Downloads/Hoppy`; notification "Ricevuti…" opens the folder on click; no answer for
   110 s = declined; a second offer during the alert is declined.
5. **Send**: Finder → right-click 1 file and 20 files → Services → *Invia con Hoppy* (also
   with Hoppy not running: it starts and then sends); drop on the app icon; *Open With →
   Hoppy*; menu *Invia file…*. Device menu at the pointer, *Cerca di nuovo*, *Annulla*;
   progress in the second status item; jingle once on delivery and none when muted;
   *Annulla invio* from the second icon; "Non riesco a raggiungere" balloon with the
   receiver off.
6. **Login item**: first run turns it on (macOS 13+: System Settings → General → Login Items
   lists Hoppy; macOS 12: the LaunchAgent plist exists); toggle off/on from the menu; log
   out and in; `hoppy uninstall` then `hoppy install`. From the .dmg (translocated): the
   toggle explains "move to Applications".
7. **Single instance**: open Hoppy again while running → "Hoppy è già attivo" notification,
   one bunny.
8. **Firewall**: turn the macOS firewall on; first receive prompts; *Deny* → a phone can
   still deliver via callback ("provo un'altra strada…" on the sender).
9. **Update notice** (release build, older version installed): notification + *Scarica
   Hoppy X…* open the portal.
10. **Notifications off** in System Settings: no notification at all; turning them back on
    restores them.

## iPhone / iPad (planned)
- Target: devices from 2021 onwards (e.g. iPhone 13 family, iPad 9th gen, iPad mini 6).
- Wi-Fi Aware (iOS 26) gives direct peer-to-peer without a shared network on iPhone 12 and later.
- Distribution only through TestFlight / App Store (no side-loading from the portal).
- Background receiving is limited: the app must be open, or woken by a push notification.
