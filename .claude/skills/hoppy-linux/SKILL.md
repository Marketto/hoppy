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
| zenity/kdialog argv builders | `crates/hoppy-cli/src/linux/dialog.rs` |
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
- Network text in notifications: `display_*` first; `notify.rs` escapes markup; zenity gets
  `--no-markup`, kdialog look-alikes for `<`/`>`; dialog list entries never start with `-`
  and never follow a bare `--` (zenity shows it as a row).
- Never auto-accept: no notification buttons and no dialog program = declined.
- Integration files carry `integration::MARKER`; never overwrite or delete a file without it.
- Asset names are a contract (`docs/release.md`): `Hoppy-linux-amd64.deb`, `SHA256SUMS.txt`.

## Verify

`python3 tools/dev.py check` (Linux unit tests included), `cargo build --release -p hoppy`,
`cargo deb -p hoppy --no-build && dpkg-deb -c target/debian/*.deb`. Scripted end-to-end
runs need a private session bus (`dbus-run-session`) and a notification server/tray host
(a fake one is fine; GNOME-specific behaviour still needs a real desktop: see the test
plan in `docs/platforms/linux.md`). Keep Windows compiling: the `windows-latest` CI job.
