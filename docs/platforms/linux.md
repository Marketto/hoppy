# Linux desktop (Ubuntu first)

The same experience as on Windows (ADR 0007): a panel icon that receives, "Send with Hoppy"
on right-click, a device list, notifications, the delivery jingle and an update check.
Target: **Ubuntu 22.04+ x64** (GNOME with the preinstalled AppIndicator extension); also
works on Kubuntu/KDE Plasma, Linux Mint (Cinnamon), and any desktop with a
StatusNotifierItem host and a notification server. macOS has its own backend with the same
UX (`docs/platforms/macos-ios.md`, ADR 0008).

## Install

- **Package**: `Hoppy-linux-amd64.deb` from the release page (portal: Ubuntu button). Open it
  with App Center / Software Install, or `sudo apt install ./Hoppy-linux-amd64.deb`.
  Installs `/usr/bin/hoppy`, `/usr/bin/hoppyw`, the app launcher
  (`/usr/share/applications/hoppy.desktop`, from `apps/linux/hoppy.desktop`) and the icon
  (hicolor 48/128/256/512 PNG + scalable SVG, from `assets/brand/linux/`). Recommends
  `zenity | kdialog`, `curl | wget`, `xdg-utils` (all preinstalled on Ubuntu desktop).
- **First run**: start *Hoppy* from the applications once. The tray then writes the per-user
  pieces below (once per version) and comes back at every login.
- **From source**: `cargo build --release -p hoppy`, then `target/release/hoppy install`
  (needs `hoppyw` next to `hoppy`; also adds a per-user launcher, since there is no system one).
- **Remove**: run `hoppy uninstall` **before** `sudo apt remove hoppy` (as each user who
  used Hoppy): it removes the per-user files and stops the tray from adding them back. The
  package has no `prerm` cleanup (it must not edit users' homes), so removing it first
  leaves those files behind: `TryExec`/`Dependencies` hide the launcher, autostart, Dolphin
  and Nemo entries, but the Nautilus script stays until deleted by hand.

## Using it

| | |
| --- | --- |
| **Receive** | `hoppyw` runs at login as a panel icon (top bar, right). An offer shows a notification with **Accept / Decline** (critical urgency: stays until answered) and, for a device not verified yet, the 4-emoji code with names and digits in its text plus a **Verify…** button; clicking **Verify…** or the body opens the verification dialog (see [Device verification](#device-verification)). No answer in 110 s, dismissed, or no way to ask = declined; never auto-accepted. Files go to `~/Downloads/Hoppy` (`Scaricati/Hoppy`); a notification "Received N file(s) from X", click = open the folder. |
| **Send (Files/Nautilus)** | select files → right-click → **Scripts → Send with Hoppy** ("Invia con Hoppy" in Italian). |
| **Send (Dolphin, Nemo)** | right-click → **Send with Hoppy** (top level). |
| **Send (launcher)** | *Open with → Hoppy*, drop files on the Hoppy icon in the dock, the launcher's *Send files…* action, or tray menu → *Send files…* (file picker). |
| **Then** | "Looking for devices…" notification (with **Cancel**) → a list dialog with nearby devices (✓ verified, 🔒 encrypted, "⚠ not encrypted" for old apps; + *Search again*) → for a device not verified yet, a code window (same 4 emoji as on the other screen, **123**, **They match**) while it decides → progress in a notification updated in place (at most every 2 s) with a **Cancel sending** button → "Delivered to X ✓" + jingle (unless muted) → the process exits 7 s later. |
| **Tray menu** | header "Hoppy · receiving as NAME", *Send files…*, *Open received folder*, *♪ Sound when delivered* (check), *Trusted devices…*, *About & diagnostics…* (log in the text editor), *Quit*; a bold *Update to X…* (installed from the .deb) or *Download Hoppy X…* (other copies) entry on top when an update exists. Left click opens it too. |

Strings: Italian when the message locale is Italian (`LANGUAGE`/`LC_ALL`/`LC_MESSAGES`/`LANG`),
else English (`gui.rs` string table, shared with Windows).

## How it works

`hoppyw` is one cross-platform GUI binary: `crates/hoppy-cli/src/gui.rs` holds the logic for
both systems and calls `platform::ui`, which is `platform::win` on Windows and
`crates/hoppy-cli/src/linux/ui.rs` here (same API; `ui::NATIVE_POPUPS = false` switches the
two places where the UIs differ). Everything is pure Rust; no C libraries beyond libc.

| Piece | File | Notes |
| --- | --- | --- |
| Panel icon + menu | `crates/hoppy-cli/src/linux/sni.rs` | StatusNotifierItem via `ksni` (zbus). Menu kept current with `Tray::set_menu`; picks → `TrayEvent::Menu(id)`; "about to show" → `TrayEvent::MenuOpening` (re-read device.json). Registers later if the host appears after us (GNOME loads extensions after autostart). No host → logged, receiving continues. |
| Notifications | `crates/hoppy-cli/src/linux/notify.rs` | `org.freedesktop.Notifications` over zbus: actions, `urgency`, `transient`, `suppress-sound`, `replaces_id` updates. Body text is markup-escaped when the server renders markup. One dispatcher thread routes `ActionInvoked`/`NotificationClosed`. |
| Dialogs | `crates/hoppy-cli/src/linux/dialog.rs` | `zenity` (default) or `kdialog` (KDE). argv only, values in `--opt=value` form, list entries never start with `-`, `--no-markup` for zenity text, `<`/`>` swapped for look-alikes for kdialog (rich-text guess). The verification dialog is the one zenity text with Pango markup (every value escaped by `markup_escape`). |
| Single instance | `crates/hoppy-cli/src/linux/ui.rs` | session-bus name `app.hoppy.Tray`; a second `hoppyw` calls its `Reopen` method → "already running" notification. |
| Jingle | `crates/hoppy-cli/src/linux/sound.rs` | `assets/music/desktop_done.wav` embedded, written to `$XDG_RUNTIME_DIR/hoppy-done-<pid>.wav`, played by `pw-play` → `paplay` → `aplay`. |
| Updates | `crates/hoppy-cli/src/unix/http.rs` (shared with macOS), `crates/hoppy-cli/src/update.rs`, `crates/hoppy-cli/src/unix/selfupdate.rs` | `curl` (redirects to https only) / `wget` (no redirects); 30 s after start, then daily. A release counts only once `Hoppy-linux-amd64.deb` and `SHA256SUMS.txt` are attached. See [Updates](#updates). |
| Right-click, launcher, autostart | `crates/hoppy-cli/src/linux/integration.rs` | see below |
| Log | `crates/hoppy-cli/src/diag.rs` | `~/.local/share/hoppy/hoppy.log` (`hoppy log`). Same privacy rules as Windows. |

## Device verification

Protocol v1 (docs/protocol.md §7, ADR 0010); the dialogs are described by
`verify::DialogView` (shared with Windows and macOS) and drawn by zenity
(`ui::show_dialog`, `dialog::dialog_args`):

- **Code**: the 4 emoji large (Noto Color Emoji renders them in colour), their names under
  them, and an extra button **123** that re-opens the dialog with the same value as
  8 digits ("1234 5678"; then **Emoji**).
- **Receiver**: buttons **Decline** (zenity's Cancel with `--default-cancel`: Enter declines),
  **Accept**, **They match, accept** (accept + mark verified). From a phone
  (`android`/`ios`) the dialog also shows a QR code drawn with Unicode half blocks in a
  black-on-white monospace span (scans like a picture, light or dark theme) — "Or scan this
  code with Hoppy on the phone"; when the phone confirms it (spec §7.7) the dialog re-opens
  with "✓ Verified: the phone confirmed the code" instead. zenity cannot change an open
  dialog, so toggles and ✓ close and re-open it.
- **Sender**: code window with **Close** (default) and **They match**; closed by itself when
  the transfer ends. No QR code (the one-time code would live in the short-lived send
  process; the phone's confirmation reaches the tray).
- **Key changed**: warning dialog with **Close** (default) and **Forget device**, on the
  sender (send refused) and on the receiver (declined without asking).
- **Trusted devices…** (tray menu): list dialog of remembered devices (✓ = verified, start
  of the key fingerprint); picking one asks "Forget X?".
- Notifications for: a phone that verified this device, a device forgotten, sessions
  abandoned mid-handshake (at most one a minute), a damaged `trust.json` set aside, trust
  changes that could not be saved; a new device reusing a known name is flagged in the
  accept text.
- **kdialog only** (no zenity): the accept question shows the code as text (emoji, names,
  digits) with Accept/Decline; no QR code, no *They match* (use `hoppy trust verify`).
  No dialog program at all: the notification carries the code as text.

## Updates

Only a copy installed from the .deb (`/usr/bin/hoppyw`, `hoppy` package registered with
dpkg) updates itself; others (built from source) get *Download Hoppy X…*, which opens the
portal.

1. *Update to X…* (menu or notification) downloads `Hoppy-linux-amd64.deb` of **that**
   release (`releases/download/vX.Y.Z/`, not `latest`) into a fresh private folder
   (`$TMPDIR/Hoppy-update-<random>`, 0700), hashing it while it is written; it is kept only
   if its SHA-256 matches the line in the same release's `SHA256SUMS.txt`.
2. "Installing Hoppy X…" notification, then `/usr/bin/pkexec /usr/lib/hoppy/install-update
   <sha256> <file>`: the desktop's polkit agent asks for the password ("Aggiorna Hoppy",
   action `app.hoppy.update`, `/usr/share/polkit-1/actions/app.hoppy.update.policy`). The
   root-owned helper (`apps/linux/install-update`, shipped in the .deb, 0755 root) never
   installs the user's file: it refuses symlinks and non-regular files, copies it into a
   fresh root-only folder (`mktemp -d /var/tmp/…`), checks the copy's SHA-256 against the
   argument and that `dpkg-deb -f` says `Package: hoppy`, then `apt-get install -y` that
   copy (`dpkg -i` without apt-get) and removes it. So a process of the user that swaps
   the downloaded file cannot get anything else installed as root. Every program is called
   by absolute path, never from `PATH` (a fake `pkexec` there could phish the password).
   Dismissed (exit 126) = "Update cancelled", nothing changed.
3. Installed: the tray starts a tiny helper (`/bin/sh`, values as positional parameters)
   that waits for the tray to exit, then starts `/usr/bin/hoppyw` again; the tray exits.
4. No `pkexec` or helper, or no polkit agent (exit 127): the .deb is opened with the
   system's software installer (`/usr/bin/xdg-open`), which shows the package and asks for
   the password itself (lower risk: the user sees what is installed; the file could still be
   swapped before it opens), and a notification says to finish there. Any other
   failure: "Update failed" (click = the portal).

### Per-user files (`hoppy install`, tray first run of each version)

| File | For |
| --- | --- |
| `~/.local/share/nautilus/scripts/Invia con Hoppy` (or `Send with Hoppy`) | Nautilus: *Scripts* submenu (a top-level entry needs a nautilus-python extension: roadmap) |
| `~/.local/share/nemo/actions/hoppy.nemo_action` | Nemo (Mint), top level |
| `~/.local/share/kio/servicemenus/hoppy.desktop` (executable) | Dolphin (KDE Frameworks 5.85+), `X-KDE-Priority=TopLevel` |
| `~/.local/share/applications/hoppy.desktop` | launcher, only when the .deb's is missing (source installs) |
| `~/.config/autostart/hoppy.desktop` | tray at login (`X-GNOME-Autostart-Delay=5`); created only by `hoppy install` and the very first tray run; upgrades refresh it only if it still exists and leave it alone when disabled (`Hidden=true`, `X-GNOME-Autostart-enabled=false`) |

Each carries the line `# Written by Hoppy (hoppy install); removed by hoppy uninstall.`;
install never overwrites a file without it, uninstall removes only files with it.
`device.json` field `desktop_integration` = the version that wrote them, or `"off"` after
`hoppy uninstall` (the tray then leaves them alone; `hoppy install` turns it back on).
Per-user autostart instead of `/etc/xdg/autostart`: it can be removed per user (GNOME
*Startup Applications*, `hoppy uninstall`) and only starts for users who opened Hoppy.
Dev builds (`cargo run`, debug) never write these by themselves.

## Firewall

Ubuntu's `ufw` is **off** by default, so nothing is needed. If it is on:
`sudo ufw allow 47471/tcp && sudo ufw allow 47470/udp`. Even without that rule, a phone or PC
can still send here: when its direct connection fails it asks Hoppy to connect back
(callback, ADR 0006), which only needs outbound traffic. `hoppy firewall` is Windows-only.

## Troubleshooting

- **No icon in the top bar** (GNOME without the AppIndicator extension, e.g. Fedora/Debian):
  Hoppy still receives (notifications) and the right-click entries work. Install/enable
  "AppIndicator and KStatusNotifierItem Support". The log says "no panel icon host".
- **No device list / file picker**: install `zenity` (or `kdialog`); a notification says so.
- **Dialog opens behind windows** (Wayland focus-stealing prevention): it is listed in the
  dock/Alt+Tab; a known limit of external dialog programs.
- **No sound**: needs `pw-play`, `paplay` or `aplay`; toggle with *♪ Sound when delivered* or
  `hoppy music on|off`.

## Device test plan (real Ubuntu desktop)

Automated here (`cargo test`, plus scripted D-Bus runs with a fake notification server and
tray host): see `docs/decisions/0007-linux-desktop.md`. On a real Ubuntu 22.04 and 24.04
(GNOME, Wayland and X11), and once on Kubuntu and Mint:

1. Install the .deb from App Center; icon in the app grid; first start → panel icon, header
   with the device name; log out/in → icon back by itself.
2. Phone → PC: Accept/Decline notification (also on the lock screen / in Do Not Disturb),
   body click → dialog (also on KDE without zenity: does `kdialog --warningyesno` focus No?), decline, 2 offers at once (second declined), files in Scaricati/Hoppy,
   click "Received" → Files opens the folder.
3. Files: right-click 1 and 20 files → Scripts → Invia con Hoppy → device list → progress
   notification (does GNOME re-show the banner on each update?) → *Annulla invio* →
   delivered + jingle once; mute via the menu check.
4. Dolphin/Nemo top-level entry; *Open with → Hoppy*; drop on the dock icon; launcher action.
5. *About & diagnostics…* opens the log; *Quit*; second launch → "already running".
6. Update (.deb installed, older version): bold *Update to X…* + notification → download
   % in the tooltip → "Installing…" → polkit password prompt → tray gone and back on the new
   version (`hoppy --version`, About); cancel the prompt → "Update cancelled"; a copy run
   from `target/release` → *Download Hoppy X…* opens the portal; Kubuntu: KDE's polkit
   agent; a session without an agent → the .deb opens in App Center.
7. Verification (GNOME light + dark theme, Kubuntu): phone → PC: notification shows the
   code and **Verify…** → dialog with the 4 colour emoji + names, **123** toggles to digits,
   QR code scans with the phone's Hoppy and turns into ✓; **They match, accept** → ✓ next
   to the phone in the send list next time. PC → PC: code window on the sender matches the
   receiver's dialog. Reinstall the other device → warning with **Forget device** on both
   sides; *Trusted devices…* lists and forgets. Orca reads the names/digits.
8. ufw on without rules: phone → PC still arrives (callback).
