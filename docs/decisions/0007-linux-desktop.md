# 0007. Linux desktop: one `hoppyw` for Windows and Linux, freedesktop pieces only
Status: Accepted (PoC)
Date: 2026-10-15

## Context
After Windows and Android, the next platform is Ubuntu, with the same UX as Windows: a
receiver icon at login, "Send with Hoppy" on right-click, a device list, notifications,
the delivery jingle, an update check. Ubuntu 22.04+ ships GNOME with the AppIndicator
extension enabled, a notification server with actions, `zenity`, PipeWire/PulseAudio
players, `wget`/`curl` and `xdg-open`. The .deb must run on 22.04 (glibc 2.35) and should
not pull C libraries (libdbus, GTK, libappindicator) or an async runtime into our code.

Options considered for the UI: a GTK4/libadwaita app (big C dependency, separate code
path, its own tray story), the `tray-icon` crate (GTK + libappindicator under the hood),
or StatusNotifierItem + freedesktop notifications over D-Bus in pure Rust (`ksni`, `zbus`)
with zenity/kdialog for the few dialogs.

## Decision
- `hoppyw` becomes cross-platform. `gui.rs` keeps all logic and talks to `platform::ui`:
  `platform::win` on Windows, `crate::linux::ui` elsewhere, with the same API. Differences
  are two flags in the API, not `cfg` in `gui.rs`: `NATIVE_POPUPS` (Linux: persistent tray
  menu via `Tray::set_menu`, picks as `TrayEvent::Menu`; popups are dialogs with their own
  Cancel) and `TrayEvent::MenuOpening`. The non-Windows stub is gone; `gui.rs` now runs
  (and is tested) on Linux.
- Tray: StatusNotifierItem via `ksni` (zbus, `async-io` executor thread, no tokio), with
  `assume_sni_available` so a host that loads after us still gets the icon; no host = keep
  receiving with notifications only.
- Notifications via zbus directly (actions, urgency, replaces_id, signals). The accept
  question is a critical notification with Accept/Decline; body click → zenity/kdialog
  question; no way to ask → declined (invariant 3).
- Send: zenity/kdialog list for devices and file picking; progress as a transient
  notification updated in place with a "Cancel sending" button.
- Integration: per-user Nautilus script, Nemo action, Dolphin service menu, launcher (only
  without the .deb's) and autostart, written by `hoppy install` and by the tray once per
  version; `hoppy uninstall` removes them and turns this off. Files carry a marker; files
  without it are never touched.
- Updates: notify and link to the portal only (no download/install: a .deb needs root and
  the system installer). Release asset `Hoppy-linux-amd64.deb`, built on ubuntu-22.04 by
  `release.yml` (no longer gated).

## Consequences
One code path for both desktops: the tray/send logic, string table and update logic are
exercised by Linux CI. hoppyw grows to ≈ 3.8 MB on Linux (zbus). zenity/kdialog are soft
dependencies (Recommends); without them sending from the right-click menu cannot pick a
device (a notification says so). Nautilus puts scripts in a *Scripts* submenu; a top-level
entry needs a nautilus-python extension (roadmap). Dialogs from a background process can
open behind other windows under Wayland focus rules. Tested here: unit tests, and scripted
runs in a private D-Bus session against a fake notification server + StatusNotifierWatcher
(menu read and clicked over dbusmenu, accept/decline/cancel actions, single instance,
fallbacks); a real GNOME/KDE session is still needed (`docs/platforms/linux.md`).

## Amendment (2026-10-15): self-update and device verification
The product owner asked that the desktop apps fetch, verify and install new versions
themselves. A copy installed from the .deb (`/usr/bin/hoppyw`) now downloads the release's
.deb, checks its SHA-256 against `SHA256SUMS.txt` and installs it with `pkexec apt-get
install -y` (the user types the password in the desktop's polkit dialog; no silent root),
then restarts; without pkexec or a polkit agent the .deb is handed to the software installer.
Other copies keep the portal link. The protocol v1 verification UI (ADR 0010) uses zenity
with Pango markup for the large emoji and a QR code drawn with Unicode half blocks; zenity
dialogs cannot change while open, so the code toggle and the ✓ after a QR scan re-open them.
Details: `docs/platforms/linux.md` "Updates" and "Device verification".
