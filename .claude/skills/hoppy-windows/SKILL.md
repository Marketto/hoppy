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
  tooltip → final balloon → the process exits a few seconds later.
  Alternative: click the tray icon → *Send files…* (native Open dialog).

## Pieces

- `crates/hoppy-cli/src/main.rs` — `hoppy` CLI (console): `send`, `receive`, `peers`,
  `name`, `install`, `uninstall`. Kept intact for scripts, tests, Linux/macOS, and as a
  fallback (`send --pause` when `hoppyw.exe` is missing).
- `crates/hoppy-cli/src/bin/hoppyw.rs` — GUI-subsystem binary; calls `hoppy::gui::run`.
  Off Windows it prints that it is Windows-only and exits 1.
- `crates/hoppy-cli/src/lib.rs` — shared helpers (`start_discovery`, `bind_receiver`,
  `default_receive_dir`, `human`, `parse_direct`, …).
- `crates/hoppy-cli/src/gui.rs` — tray receiver + send flow, IT/EN string table
  (`GetUserDefaultUILanguage`), safe Rust only. Worker threads update `Mutex` state and call
  `Waker::wake` (→ `WM_APP` → `TrayEvent::Wake`); only the UI thread touches the tray.
- `crates/hoppy-cli/src/platform.rs` — **all Win32 FFI** (`windows-sys` 0.61) in
  `#[cfg(windows)] mod win`: hidden window + `Shell_NotifyIconW` (NOTIFYICON_VERSION_4,
  re-added on `TaskbarCreated`), popup menus, `GetOpenFileNameW`, `ShellExecuteW`, named
  mutex `Local\HoppyTray` (single instance; a 2nd launch makes the 1st show a balloon), and
  `install()`. A non-Windows stub with the same API makes `gui.rs` compile and lint on Linux.
  `install()` writes two `.lnk` via PowerShell `WScript.Shell` (values in env vars, never
  string-interpolated): `%APPDATA%\Microsoft\Windows\SendTo\Hoppy.lnk` → `hoppyw.exe send`
  and `...\Startup\Hoppy receiver.lnk` → `hoppyw.exe`.
- `crates/hoppy-cli/build.rs` — embeds `assets/brand/hoppy.ico` as icon resource **id 1** in
  both exes (the tray loads it with `LoadImageW`).
- `apps/windows/hoppy.iss` — Inno Setup: installs both exes, kills running copies in
  `PrepareToInstall` (upgrade from the 0.1.0 console receiver), Start menu "Hoppy" → `hoppyw.exe`.
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
- `&` in menu text is an accelerator: escape device names (`&&`), done in `popup_menu`.
- Each exe path gets its own firewall rule: the first `hoppyw` receive triggers the prompt
  again even if `hoppy.exe` was allowed; the user must allow **Private** networks.
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
Manual test on a real PC: tray icon at login, accept dialog, balloon click, Send to menu.
