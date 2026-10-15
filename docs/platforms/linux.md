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
| **Receive** | `hoppyw` runs at login as a panel icon (top bar, right). An offer shows a notification with **Accept / Decline** (critical urgency: stays until answered); clicking its body opens a question dialog with the full text (zenity whenever installed, also on KDE, since it defaults to No; else `kdialog --warningyesno`). No answer in 110 s, dismissed, or no way to ask = declined; never auto-accepted. Files go to `~/Downloads/Hoppy` (`Scaricati/Hoppy`); a notification "Received N file(s) from X", click = open the folder. |
| **Send (Files/Nautilus)** | select files → right-click → **Scripts → Send with Hoppy** ("Invia con Hoppy" in Italian). |
| **Send (Dolphin, Nemo)** | right-click → **Send with Hoppy** (top level). |
| **Send (launcher)** | *Open with → Hoppy*, drop files on the Hoppy icon in the dock, the launcher's *Send files…* action, or tray menu → *Send files…* (file picker). |
| **Then** | "Looking for devices…" notification (with **Cancel**) → a list dialog with nearby devices (+ *Search again*) → progress in a notification updated in place (at most every 2 s) with a **Cancel sending** button → "Delivered to X ✓" + jingle (unless muted) → the process exits 7 s later. |
| **Tray menu** | header "Hoppy · receiving as NAME", *Send files…*, *Open received folder*, *♪ Sound when delivered* (check), *About & diagnostics…* (log in the text editor), *Quit*; a bold *Download Hoppy X…* entry on top when an update exists. Left click opens it too. |

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
| Dialogs | `crates/hoppy-cli/src/linux/dialog.rs` | `zenity` (default) or `kdialog` (KDE). argv only, values in `--opt=value` form, list entries never start with `-`, `--no-markup` for zenity text, `<`/`>` swapped for look-alikes for kdialog (rich-text guess). |
| Single instance | `crates/hoppy-cli/src/linux/ui.rs` | session-bus name `app.hoppy.Tray`; a second `hoppyw` calls its `Reopen` method → "already running" notification. |
| Jingle | `crates/hoppy-cli/src/linux/sound.rs` | `assets/music/desktop_done.wav` embedded, written to `$XDG_RUNTIME_DIR/hoppy-done-<pid>.wav`, played by `pw-play` → `paplay` → `aplay`. |
| Update check | `crates/hoppy-cli/src/unix/http.rs` (shared with macOS), `crates/hoppy-cli/src/update.rs` | `curl` (redirects to https only) / `wget` (no redirects); 30 s after start, then daily. A release counts only once `Hoppy-linux-amd64.deb` and `SHA256SUMS.txt` are attached. Linux **never downloads or installs**: the tray shows *Download Hoppy X…* and a notification that open the portal. Installing a .deb needs the user's password and the system installer, which already verifies the package; a silent self-update would need root. |
| Right-click, launcher, autostart | `crates/hoppy-cli/src/linux/integration.rs` | see below |
| Log | `crates/hoppy-cli/src/diag.rs` | `~/.local/share/hoppy/hoppy.log` (`hoppy log`). Same privacy rules as Windows. |

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
6. Update: with an older version installed, the bold *Download Hoppy X…* entry + notification
   open the portal.
7. ufw on without rules: phone → PC still arrives (callback).
