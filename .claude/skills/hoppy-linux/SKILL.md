---
name: hoppy-linux
description: Use for the Ubuntu/Linux desktop experience - hoppyw on Linux (panel icon via StatusNotifierItem, notifications, zenity/kdialog dialogs, jingle, update check), the Nautilus/Nemo/Dolphin "Send with Hoppy" entries, autostart, and the .deb package.
---

# Linux desktop

UX, files and test plan: `docs/platforms/linux.md`. Decision: `docs/decisions/0007-linux-desktop.md`.

## Where things are

| What | File |
| --- | --- |
| Shared tray/send logic + IT/EN strings (also Windows) | `crates/hoppy-cli/src/gui.rs` |
| `platform::ui` on Linux (same API as `platform::win`) | `crates/hoppy-cli/src/linux/ui.rs` |
| Panel icon (ksni) | `crates/hoppy-cli/src/linux/sni.rs` |
| Notifications (zbus) | `crates/hoppy-cli/src/linux/notify.rs` |
| zenity/kdialog argv builders (incl. the verification dialog with Pango markup) | `crates/hoppy-cli/src/linux/dialog.rs` |
| Send window (wry/tao on WebKitGTK, shared with Windows/macOS; ADR 0011) | `crates/hoppy-cli/src/sendui/` |
| Theme choice + OS signals (`gsettings`) | `crates/hoppy-cli/src/theme.rs`, `crates/hoppy-cli/src/linux/mod.rs` (`system_look`) |
| Verification dialog model (shared) | `crates/hoppy-cli/src/verify.rs` |
| Self-update (.deb via pkexec, Mac app swap, relaunch helper) | `crates/hoppy-cli/src/unix/selfupdate.rs` |
| Update check cadence (shared, all desktops) | `crates/hoppy-cli/src/update.rs` (`Cadence`, `check_due`: 30 s, every 3 h, after wake-up, on menu open with a 10-min throttle) + `gui.rs` (`request_update_check`; *About…* dialog `about_dialog` with *Check for updates*) |
| Per-user right-click entries, launcher, autostart | `crates/hoppy-cli/src/linux/integration.rs` |
| Jingle players / curl+wget | `crates/hoppy-cli/src/linux/sound.rs`, `crates/hoppy-cli/src/unix/http.rs` (shared with macOS) |
| Package launcher (kept equal to `integration::launcher_entry` by a test) | `apps/linux/hoppy.desktop` |
| Icons (generated: `python3 tools/gen_icons.py --linux`) | `assets/brand/linux/` |
| .deb metadata | `crates/hoppy-cli/Cargo.toml` (`[package.metadata.deb]`) |
| Release job (`Hoppy-linux-amd64.deb`, ubuntu-22.04) | `.github/workflows/release.yml` |

## Rules

- `gui.rs` stays platform-neutral: no `cfg` attributes there. Differences go behind the
  `platform::ui` API (`NATIVE_POPUPS`, `Tray::set_menu`, `TrayEvent::Menu`/`MenuOpening`);
  a new `ui` function needs a Windows twin in `platform::win` (keep it tiny) and a macOS
  twin in `crate::macos::ui` (`.claude/skills/hoppy-macos/SKILL.md`), or a runtime
  `cfg!(…)` check.
- `crate::linux` is compiled on every non-Windows, non-macOS target (two `cfg` attributes in
  `lib.rs`); Linux-only crates go in
  `[target.'cfg(all(not(windows), not(target_os = "macos")))'.dependencies]`. Code shared
  with macOS (`which`, curl/wget) lives in `crate::unix`.
- No tokio: zbus/ksni use `async-io` (their own thread); our code is blocking.
- `hoppyw` links WebKitGTK 4.1 + GTK 3 for the send window (build: `libwebkit2gtk-4.1-dev
  libgtk-3-dev`; .deb Depends `libwebkit2gtk-4.1-0`); the `hoppy` CLI must not (check with
  `readelf -d target/release/hoppy`). The window falls back to zenity when it cannot start.
- Network text in notifications: `display_*` first; `notify.rs` escapes markup; zenity gets
  `--no-markup`, kdialog look-alikes for `<`/`>`; dialog list entries never start with `-`
  and never follow a bare `--` (zenity shows it as a row).
- Never auto-accept: no notification buttons and no dialog program = declined.
- The verification dialog is the only zenity text with markup on: every value goes through
  `dialog::markup_escape`; sizes are integers (1/1024 pt) so old Pango parses them. It shows
  only the 4 emoji + names (no digits, no QR code); `run_watch` kills it when its
  `DialogLive` closes.
- Self-update only for `/usr/bin/hoppyw` from the package (`selfupdate::deb_installed`);
  root only through `/usr/bin/pkexec /usr/lib/hoppy/install-update SHA256 FILE` (root-owned
  helper `apps/linux/install-update` + polkit action `apps/linux/app.hoppy.update.policy`,
  both cargo-deb assets): root never installs the user-writable file, only its own copy
  re-hashed in a root-only folder. Programs by absolute path, never `which`/`PATH`; never
  sudo or a setuid helper; values to `/bin/sh` only as positional parameters.
- Integration files carry `integration::MARKER`; never overwrite or delete a file without it.
- Asset names are a contract (`docs/release.md`): `Hoppy-linux-amd64.deb`, `SHA256SUMS.txt`.

## Verify

`python3 tools/dev.py check` (Linux unit tests included), `cargo build --release -p hoppy`,
`cargo deb -p hoppy --no-build && dpkg-deb -c target/debian/*.deb`. Send window:
`xvfb-run -a target/debug/hoppyw --smoke-test-window` (every screen × theme) and
`python3 tools/sendui_audit.py` (axe-core in Chromium). Scripted end-to-end
runs need a private session bus (`dbus-run-session`) and a notification server/tray host
(a fake one is fine; GNOME-specific behaviour still needs a real desktop: see the test
plan in `docs/platforms/linux.md`). Real zenity renders under `Xvfb` (with
`LANG=C.UTF-8`, else zenity rejects non-ASCII arguments, and `GSK_RENDERER=cairo` for
zenity 4); `import -window root` captures the dialog to check the emoji render. Keep Windows compiling: the `windows-latest` CI job.
