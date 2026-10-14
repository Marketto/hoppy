---
name: hoppy-windows
description: Use for the Windows experience - the hoppyw tray receiver and "Send to → Hoppy" GUI, the hoppy CLI commands, installer script, firewall, startup receiver, and the future native Windows UI/shell extension.
---

# Windows

PoC UX (no console windows):
- **Receive**: `hoppyw.exe` starts at login as an icon in the notification area (near the
  clock). Each incoming offer asks with a native Yes/No dialog (`MessageBoxW`); files go to
  `Downloads\Hoppy`; a balloon/toast says "Ricevuti N file da …", clicking it opens the folder.
- **Send**: Explorer → right click → (Win11: *Show more options*, or Shift+right-click) →
  **Send to → Hoppy** runs `hoppyw send FILES`: tray icon "Looking for devices…", then a
  popup menu at the mouse cursor lists nearby devices → click one → progress in the tray
  tooltip → final balloon → the process exits a few seconds later. Music (sender only,
  unless `music: false` in device.json): idle loop → transfer loop once accepted → done
  jingle; stopped on decline/error/cancel/exit. Rules: `docs/design/themes.md` "Music".
  Alternative: click the tray icon → *Send files…* (native Open dialog).

## Pieces

- `crates/hoppy-cli/src/main.rs` — `hoppy` CLI (console): `send`, `receive`, `peers`,
  `name`, `music [on|off]`, `install`, `uninstall`. The CLI itself never plays sound. Kept intact for scripts, tests, Linux/macOS, and as a
  fallback (`send --pause` when `hoppyw.exe` is missing).
- `crates/hoppy-cli/src/bin/hoppyw.rs` — GUI-subsystem binary; calls `hoppy::gui::run`.
  Off Windows it prints that it is Windows-only and exits 1.
- `crates/hoppy-cli/src/lib.rs` — shared helpers (`start_discovery`, `bind_receiver`,
  `default_receive_dir`, `human`, `parse_direct`, …).
- `crates/hoppy-cli/src/gui.rs` — tray receiver + send flow, IT/EN string table
  (`GetUserDefaultUILanguage`), safe Rust only. Worker threads update `Mutex` state and call
  `Waker::wake` (→ `WM_APP` → `TrayEvent::Wake`); only the UI thread touches the tray.
- `crates/hoppy-cli/src/update.rs` — tray update check, pure and Linux-tested: semver
  compare with `CARGO_PKG_VERSION`, `releases/latest` JSON, `SHA256SUMS.txt`, install
  location (`%LOCALAPPDATA%\Programs\Hoppy` = setup, else portable → portal link), and
  `download_fresh` (new `%TEMP%\Hoppy-update-<uuid>` folder via `create_dir`; `*.part`,
  hashed while written, renamed only if the SHA-256 matches; stale folders >1 day removed
  at tray start by `remove_stale_update_dirs`) over an
  `Http` trait (fake in tests). `gui.rs` runs it on worker threads (30 s, then daily),
  shows the bold "Update to X…" item / balloon, runs the installer with `/UPDATE=1` and
  exits. Asset names are a contract with `release.yml` (docs/release.md).
- `crates/hoppy-cli/src/platform.rs` — **all Win32 FFI** (`windows-sys` 0.61) in
  `#[cfg(windows)] mod win`: hidden window + `Shell_NotifyIconW` (NOTIFYICON_VERSION_4,
  re-added on `TaskbarCreated`), popup menus, `GetOpenFileNameW`, `ShellExecuteW`, named
  mutex `Local\HoppyTray` (single instance; a 2nd launch makes the 1st show a balloon),
  `http_get` (WinHTTP, `WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY`, https only via
  `split_https_url`; bold menu items = `SetMenuDefaultItem`), and
  `install()`, and `play_tune`/`stop_tune` (`PlaySoundW` + `SND_MEMORY|SND_ASYNC`, WAVs
  from `assets/music/` embedded with `include_bytes!` only in Windows builds). A non-Windows stub with the same API makes `gui.rs` compile and lint on Linux.
  `install()` writes two `.lnk` via PowerShell `WScript.Shell` (values in env vars, never
  string-interpolated): `%APPDATA%\Microsoft\Windows\SendTo\Hoppy.lnk` → `hoppyw.exe send`
  and `...\Startup\Hoppy receiver.lnk` → `hoppyw.exe`.
- `crates/hoppy-cli/build.rs` — embeds `assets/brand/hoppy.ico` as icon resource **id 1** in
  both exes (the tray loads it with `LoadImageW`).
- `apps/windows/hoppy.iss` — Inno Setup: installs both exes, stops copies running from
  `{app}` (PowerShell, path-filtered) in `PrepareToInstall` (upgrade from the 0.1.0 console
  receiver) and on uninstall (`usUninstall`), Start menu "Hoppy" → `hoppyw.exe`. Wizard
  music: WAVs as `dontcopy` files → `ExtractTemporaryFile` in `InitializeWizard`, winmm
  `PlaySoundW` imported twice (String name / Longint 0 = stop), skipped if `WizardSilent`,
  stopped in `DeinitializeSetup`. `[Run]` relaunches `hoppyw.exe` when `IsUpdate`
  (`/UPDATE=1`, passed by the tray updater with `/VERYSILENT`); if the update fails or
  stops before `ssPostInstall`, `DeinitializeSetup` relaunches `{app}\hoppyw.exe` instead. Keep every section (`grep -n '^\[' hoppy.iss`) after edits.
- `apps/windows/install.ps1` — portable zip install into `%LOCALAPPDATA%\Programs\Hoppy`.
- Docs: `docs/platforms/windows.md`.

## Gotchas

- Windows 11 shows "Send to" under "Show more options" (or Shift+right-click). A top-level
  context-menu entry needs an `IExplorerCommand` handler in a signed MSIX package; appearing
  in the Win11 **Share** sheet ("Condividi") also needs a packaged (MSIX) app declaring a
  share target. Both are on the roadmap.
- Tray menus: `SetForegroundWindow(hwnd)` before `TrackPopupMenu`, `PostMessage(WM_NULL)`
  after, or the menu does not close. The send menu relies on the foreground right Explorer
  hands to the process it launches.
- Network text (device names, platforms, file names, remote errors) goes through
  `display_name`/`display_platform`/`display_file_name`/`display_text` (lib.rs) before
  any dialog, menu, tooltip or balloon: no control/bidi chars, capped length.
- `&` in menu text is an accelerator: escape device names (`&&`), done in `popup_menu`.
- Each exe path gets its own firewall rule: the first `hoppyw` receive triggers the prompt
  again even if `hoppy.exe` was allowed; the user must allow **Private** networks.
- `PlaySoundW` is one sound per process: a new call replaces the current one, a null name
  stops it. `SND_MEMORY` + `SND_ASYNC` needs the buffer alive during playback ('static).
- `device.json` fields added later need `#[serde(default)]`; `config::set_music` re-reads
  the file before saving so the long-running tray never overwrites a newer name.
- `std::process::exit` skips destructors: let `Tray` drop (removes the icon) before exiting.
- Open files cannot be renamed/deleted: close handles first (see `PartFile`).
- Paths with emoji/non-ASCII: always `PathBuf`/`OsStr` (`encode_wide`/`from_wide`), never
  `String` round-trips.
- windows-sys 0.61: handles are `*mut c_void`, `BOOL` is `i32`, `PCWSTR` is `*const u16`;
  items are gated by Cargo features (check `#[cfg(feature = …)]` in the registry source).

## Verify

Linux can't build the Windows target here. Run `python3 tools/dev.py check`, then rely on
the `windows-latest` CI job (clippy + `cargo test` + artifacts `hoppy.exe`, `hoppyw.exe`);
its failures appear as annotations (`tools/ci-annotate.sh`), not only in the log.
Manual test on a real PC: tray icon at login, accept dialog, balloon click, Send to menu,
send music (idle → transfer → done, mute toggle), installer music + checkbox, update flow
(balloon → download % → silent install → tray back; portable → portal).
