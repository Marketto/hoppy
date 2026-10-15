# macOS and iPhone/iPad

## macOS

The same experience as on Windows and Ubuntu (ADR 0008): a menu-bar icon that receives,
"Send with Hoppy" from the Finder, a device picker window, notifications, the delivery
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

Every new version is a new ad-hoc signature: the privacy prompts may come back after an
update (an update installed by Hoppy itself is downloaded with curl, so it carries no
quarantine flag and Gatekeeper does not block it; a .dmg downloaded by the browser does). Running Hoppy straight from the .dmg or from Downloads works, but macOS
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
  (Return), **Accetta/Accept** the second, **Coincide, accetta / They match, accept** the
  third when there is a code to compare; no answer within 110 s = declined; a second offer
  while one is asked is declined (`gui::GuiReceiver`). Files go to `~/Downloads/Hoppy`. A
  notification "Ricevuti N file da …"; clicking it opens the folder.
- **Device verification** (protocol v1, docs/protocol.md §7): the alert's accessory view
  (`macos::ui::accessory`) shows the 4 emoji (Apple Color Emoji) with their names under
  them and a **123** button that switches to the same value as 8 digits; from a phone
  (`android`/`ios`) also the QR code (an `NSImage` from `verify::QrMatrix::bmp`, black on
  white) "Inquadra con Hoppy sul telefono", replaced by "✓ Verificato…" once the phone
  confirms it. The sender (`hoppyw send`) shows the same code in an alert with **Chiudi**
  (Return) and **Coincide** while the receiver decides (no QR code: its one-time code would
  live in the send process); it closes when the transfer ends (the send worker closes it:
  while that alert is up the menu-bar title is not updated). A changed key gives a warning
  alert with **Chiudi** (Return) and **Dimentica dispositivo**; *Dispositivi fidati…* in the
  menu lists remembered devices and forgets one. Dialogs can nest (a warning over an accept
  question); each closes only itself on timeout.
- **Send**: Finder → select files → right-click → **Services → Invia con Hoppy** (Finder
  menu → Services works too), or drop files on the Hoppy icon (Applications, Launchpad
  shortcut, Finder sidebar), or *Open With → Hoppy*, or the menu-bar icon → *Invia file…*.
  The tray starts `hoppyw send FILE…` (a second status item shows "Cerco dispositivi…",
  then the progress as its title); the devices appear in a small **picker window** near
  the mouse pointer (title "Invia «file» +N (size) a:", one button per device with its
  ✓/🔒/⚠ badge, *Cerca di nuovo*, *Annulla*; Esc or the close button = cancel). Before
  showing it the sender makes itself the active app (`activateIgnoringOtherApps`, plus
  `activate` on macOS 14+, waiting up to 1 s) and logs `activation:` / `picker:` lines.
  It is a window, not a menu at the pointer, because a popup menu from this background
  process (started by the Finder service while the Finder is in front) closed again at
  once on macOS 12 (0.7.0 bug). Silent
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
- **Updates** (`crates/hoppy-cli/src/unix/selfupdate.rs`): a release counts only once
  `Hoppy-macos-universal.dmg` and `SHA256SUMS.txt` are attached. Installed as
  `/Applications/Hoppy.app` or `~/Applications/Hoppy.app` in a folder this user can write
  (probed at tray start): *Aggiorna a X…* downloads the .dmg of **that** release
  (`releases/download/vX.Y.Z/`) with curl into a fresh private temporary folder, keeps it
  only if its SHA-256 matches `SHA256SUMS.txt`, then `hdiutil attach -nobrowse -readonly`
  on a private mount point, `ditto` Hoppy.app into a hidden `.hoppy-update-…` folder next to
  the installed app, `hdiutil detach`, checks the copy (`codesign --verify --deep --strict`,
  ad-hoc signatures pass; `CFBundleIdentifier` = `app.hoppy.desktop` and
  `CFBundleShortVersionString` = X via PlistBuddy), swaps it in with two renames (old app →
  `.hoppy-old-…`, put back if the second rename fails) and removes the old one; a helper
  (`/bin/sh`, the path as a positional parameter) waits for the tray to exit and runs `open
  -n Hoppy.app`. Errors: "Aggiornamento non riuscito" (click = portal), the installed app is
  unchanged. Elsewhere (Downloads, the .dmg, a folder without write access, a bare binary):
  notify + *Scarica Hoppy X…* → portal. Leftover hidden folders older than an hour are
  removed at tray start.
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
Gatekeeper opens Hoppy with a single confirmation and privacy grants survive updates (the
in-app updater then installs a Developer ID–signed app; `codesign --verify` already checks
it).

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
   Apri all'accesso, Dispositivi fidati…, Informazioni e diagnostica…, Esci; no Dock icon,
   no app menu.
4. **Receive**: phone → Mac: alert in front of other apps (also over a full-screen app and on
   another Space), Return = Rifiuta, Accetta saves to
   `~/Downloads/Hoppy`; notification "Ricevuti…" opens the folder on click; no answer for
   110 s = declined; a second offer during the alert is declined.
5. **Send**: Finder → right-click 1 file and 20 files → Services → *Invia con Hoppy* (also
   with Hoppy not running: it starts and then sends); drop on the app icon; *Open With →
   Hoppy*; menu *Invia file…*. The picker window appears near the pointer, in front of the
   Finder, and stays until a choice (2015 MacBook Air on macOS 12: the 0.7.0 bug); click a
   device, *Cerca di nuovo*, *Annulla*, Esc and the close button (all cancel); the log has
   `activation: active=true` and `picker: closed, choice=…`;
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
9. **Update** (release build, older version in /Applications, admin user): notification
   *Hoppy X è disponibile* → *Aggiorna a X…* → % in the title → "Installo Hoppy X…" → the
   bunny disappears and comes back on X (About, `hoppy --version`) with no Gatekeeper
   prompt; Login Items still lists Hoppy (macOS 13+) and *Apri all'accesso* is still on;
   notifications/Local Network may be asked again (new ad-hoc signature). A standard
   (non-admin) user with Hoppy in /Applications, or Hoppy run from Downloads: *Scarica
   Hoppy X…* opens the portal. Pull the network during the download → "Aggiornamento non
   riuscito", app unchanged, no `.hoppy-*` folder left in /Applications.
10. **Notifications off** in System Settings: no notification at all; turning them back on
    restores them.
11. **Verification**: phone → Mac: the alert shows the 4 colour emoji with names, *123*
    toggles to digits and back, the QR code scans with the phone's Hoppy and turns into ✓,
    *Coincide, accetta* → next time the device shows ✓ in the send menu; Mac → PC: the code
    alert matches the PC's dialog, *Coincide* → ✓; reinstall the phone → warning alert,
    *Dimentica dispositivo*; *Dispositivi fidati…*; VoiceOver reads the emoji names and the
    digits; a warning arriving while the accept alert is open stacks on top, and each one
    times out on its own.

## iPhone / iPad

See [`ios.md`](ios.md) (ADR 0009): SwiftUI app + Share extension over the Rust core, unsigned
`Hoppy-ios-unsigned.ipa` sideloaded with Sideloadly and a free Apple ID (7-day signature,
Developer Mode), unicast discovery sweep instead of multicast, receiving only while open.
