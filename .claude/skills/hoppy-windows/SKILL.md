---
name: hoppy-windows
description: Use for the Windows experience - the hoppyw tray receiver, the "Send with Hoppy" right-click verb / "Send to → Hoppy" GUI, the hoppy CLI commands, installer script, firewall, startup receiver, diagnostics log, and the future native Windows UI/shell extension.
---

# Windows

PoC UX (no console windows):
- **Receive**: `hoppyw.exe` starts at login as an icon in the notification area (near the
  clock). Each incoming offer asks with a native Yes/No dialog (`MessageBoxW`); files go to
  `Downloads\Hoppy`; a balloon/toast says "Ricevuti N file da …", clicking it opens the folder.
- **Send**: Explorer → select files → right click → **Send with Hoppy** (HKCU verb; on Win11
  top-level only with the opt-in classic menu, else *Show more options*), or **Send to →
  Hoppy**, or drop on the desktop shortcut, runs `hoppyw send FILES` (no files = Open
  dialog first; one process per file from the verb, merged by the first one): tray icon
  "Looking for devices…", then a
  popup menu at the mouse cursor lists nearby devices → click one → progress in the tray
  tooltip → final balloon → the process exits a few seconds later. Sound: silent while
  searching/menu/sending; only the done jingle on delivery (replaces the balloon's system
  sound), unless `music: false` in device.json (tray item "♪ Sound when delivered",
  `hoppy music on|off`; field name kept for compatibility). The tray receiver plays no
  jingle (balloon system sound only). Rules: `docs/design/themes.md` "Music".
  Alternative: click the tray icon → *Send files…* (native Open dialog).

## Pieces

- `crates/hoppy-cli/src/main.rs` — `hoppy` CLI (console): `send`, `receive`, `peers`,
  `name`, `music [on|off]`, `install`, `uninstall`, `classic-menu [on|off]
  [--restart-explorer]`, `log [--lines N]`, `firewall [--fix|--remove]` (these three run
  before `config::load_or_create`; for `firewall` the elevated copy may be another user). The CLI itself never plays sound. Kept intact for scripts, tests, Linux/macOS, and as a
  fallback (`send --pause` when `hoppyw.exe` is missing).
- `crates/hoppy-cli/src/bin/hoppyw.rs` — GUI-subsystem binary; calls `hoppy::gui::run`
  (also on Linux: `.claude/skills/hoppy-linux/SKILL.md`; elsewhere exits 1).
- `crates/hoppy-cli/src/lib.rs` — shared helpers (`start_discovery`, `bind_receiver`,
  `default_receive_dir`, `human`, `parse_direct`, …).
- `crates/hoppy-cli/src/gui.rs` — tray receiver + send flow, IT/EN string table
  (`GetUserDefaultUILanguage`), safe Rust only, shared with Linux: it calls `platform::ui`
  (= `win` here). The tray menu is built by `TrayMenu::items` and run by `on_pick`;
  `ui::NATIVE_POPUPS` is true on Windows (menu built per click, `Tray::set_menu` is a
  no-op, `TrayEvent::Menu`/`MenuOpening` never occur). Worker threads update `Mutex` state and call
  `Waker::wake` (→ `WM_APP` → `TrayEvent::Wake`); only the UI thread touches the tray.
  **The UI thread does no file/registry/network I/O and never waits on a lock held across
  I/O**: device.json is cached in `Prefs` (refreshed by a worker on each menu open; writes
  via `Ctx::config_job`, serialised by `CONFIG_IO`, which the UI never takes); folders/URLs,
  process spawns, UAC (`run_elevated` takes a `Waker` as owner) and the classic-menu toggle
  run on workers; worker errors become `Notice`s (balloon) + log lines. Handlers run inside
  `diag::UiWatch::handler`; wrap modal calls (menus, dialogs) in `h.modal(...)`.
- `crates/hoppy-cli/src/diag.rs` — `%LOCALAPPDATA%\Hoppy\hoppy.log` (channel → writer
  thread, rotation at 256 KB, UTC timestamps; no file names/paths: `format_line` runs
  `redact_paths`, core errors are logged via `error_kind`/`error_chain`, never `{e}`; IPs
  and device names allowed), `UiWatch` (handler > 200 ms
  logged, watchdog line > 2 s). Init only in `hoppyw` tray/send (not `firewall`: elevated
  copy may be another user). Tray menu *About & diagnostics…* opens it in Notepad.
- `crates/hoppy-cli/src/update.rs` — tray update check, pure and Linux-tested: semver
  compare with `CARGO_PKG_VERSION`, `releases/latest` JSON, `SHA256SUMS.txt`, install
  location (`%LOCALAPPDATA%\Programs\Hoppy` = setup, else portable → portal link), and
  `download_fresh` (new `%TEMP%\Hoppy-update-<uuid>` folder via `create_dir`; `*.part`,
  hashed while written, renamed only if the SHA-256 matches; stale folders >1 day removed
  at tray start by `remove_stale_update_dirs`) over an
  `Http` trait (fake in tests). `gui.rs` runs it on worker threads (30 s, then daily),
  shows the bold "Update to X…" item / balloon, runs the installer with `/UPDATE=1` and
  exits. Asset names are a contract with `release.yml` (docs/release.md).
- `crates/hoppy-cli/src/firewall.rs` — firewall rules, cross-platform std code + Linux
  tests: netsh argument lists (`add_commands`/`remove_commands`; delete only by program
  path, never by name alone — other users' installs share the names), PowerShell
  `STATUS_SCRIPT` (paths in `$env:HOPPY_EXES`) → `Status::parse` → `verdict` (tray/CLI) and
  `check_code` (`hoppyw firewall --check`, read by the installer). Rule names, ports and
  profiles (Private + Domain, never Public) are documented in docs/platforms/windows.md
  "Firewall". `gui.rs` checks at tray start, balloon once per version (`firewall_hint` in
  device.json), menu item while needed, fix = UAC `hoppyw firewall --apply`.
- `crates/hoppy-cli/src/platform.rs` — **all Win32 FFI** (`windows-sys` 0.61) in
  `#[cfg(windows)] mod win`: hidden window + `Shell_NotifyIconW` (NOTIFYICON_VERSION_4,
  re-added on `TaskbarCreated`), popup menus (at the click's anchor from `wParam`,
  `TrayEvent::Click(Some((x, y)))`; cursor otherwise), `GetOpenFileNameW` (also unowned
  for `hoppyw send` without files), `ShellExecuteW` (COM-initialised, worker-safe), the
  `hoppyw send` collector (`gather_send_files`: mutex + window `HoppySendCollector`,
  `WM_COPYDATA` from our own exe only (`wParam` = sender window → image path), capped
  by `Collected` at `COLLECT_MAX_FILES`/`COLLECT_MAX_BYTES`, timing in `collect_wait`), HKCU registry helpers (verb
  `Software\Classes\*\shell\Hoppy`, classic menu CLSID override + `Software\Hoppy`
  marker, `restart_explorer`), named
  mutex `Local\HoppyTray` (single instance; a 2nd launch makes the 1st show a balloon),
  `http_get` (WinHTTP, `WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY`, https only via
  `split_https_url`; bold menu items = `SetMenuDefaultItem`), and
  `install()`, `run_elevated` (`ShellExecuteExW` "runas", hidden, returns an `Elevated`
  process handle to wait on; UAC "No" = `ElevationDeclined`; needs the
  `Win32_System_Registry` feature for `SHELLEXECUTEINFOW`), `no_window` (CREATE_NO_WINDOW
  for netsh/powershell children of the GUI exe), and `play_done_jingle`/`stop_sound` (`PlaySoundW` + `SND_MEMORY|SND_ASYNC`;
  only `assets/music/desktop_done.wav` is embedded, with `include_bytes!`, only in Windows
  builds). On Linux `crate::linux::ui` has the same API, so `gui.rs` compiles, lints and runs
  there; a new `win` function used by `gui.rs` needs its Linux twin (and vice versa).
  `install()` writes two `.lnk` via PowerShell `WScript.Shell` (values in env vars, never
  string-interpolated): `%APPDATA%\Microsoft\Windows\SendTo\Hoppy.lnk` → `hoppyw.exe send`
  and `...\Startup\Hoppy receiver.lnk` → `hoppyw.exe`.
- `crates/hoppy-cli/build.rs` — embeds `assets/brand/hoppy.ico` as icon resource **id 1** in
  both exes (the tray loads it with `LoadImageW`).
- `apps/windows/hoppy.iss` — Inno Setup: tasks `firewall`, `classicmenu` (Win11:
  `hoppy classic-menu on`, then asks before `--restart-explorer`), `desktopicon`
  (`{userdesktop}\Hoppy` → `hoppyw.exe send`), all checked and skipped on `IsUpdate`;
  installs both exes, stops copies running from
  `{app}` (PowerShell, path-filtered) in `PrepareToInstall` (upgrade from the 0.1.0 console
  receiver) and on uninstall (`usUninstall`), Start menu "Hoppy" → `hoppyw.exe`. Wizard
  music: WAVs as `dontcopy` files → `ExtractTemporaryFile` in `InitializeWizard`, winmm
  `PlaySoundW` imported twice (String name / Longint 0 = stop), skipped if `WizardSilent`,
  stopped in `DeinitializeSetup`. Firewall task (checked, hidden when `IsUpdate`): in
  `CurStepChanged(ssPostInstall)` `hoppyw firewall --check`, then `ShellExec('runas', …
  'firewall --apply')` (declined UAC = silent False, so not `[Run]`); uninstall removes
  them the same way only when rules exist. `[Run]` relaunches `hoppyw.exe` when `IsUpdate`
  (`/UPDATE=1`, passed by the tray updater with `/VERYSILENT`); if the update fails or
  stops before `ssPostInstall`, `DeinitializeSetup` relaunches `{app}\hoppyw.exe` instead. Keep every section (`grep -n '^\[' hoppy.iss`) after edits.
- `apps/windows/install.ps1` — portable zip install into `%LOCALAPPDATA%\Programs\Hoppy`.
- Docs: `docs/platforms/windows.md`.

## Gotchas

- **Sent vs posted messages**: Explorer delivers tray callbacks (and `TaskbarCreated`) with
  `SendNotifyMessage`. `GetMessageW` runs `wndproc` for sent messages internally and does
  not return, so anything `wndproc` only queues waits for the next *posted* message (the
  0.2.1 "menu appears minutes later at a random spot" bug). `wndproc` posts `WM_KICK` to
  itself after queuing; keep that for any new queued event. Same reason the send collector
  pumps with `MsgWaitForMultipleObjects` + `PeekMessageW` (WM_COPYDATA is sent).
- Values for PowerShell go through env vars; registry values are written with the API
  (`RegSetValueExW`), never via `reg.exe` command lines.
- Windows 11 shows classic verbs and "Send to" under "Show more options" (or
  Shift+right-click) unless the per-user classic menu is on. A top-level entry in the new
  menu needs an `IExplorerCommand` handler in a signed MSIX package; appearing
  in the Win11 **Share** sheet ("Condividi") also needs a packaged (MSIX) app declaring a
  share target. Both are on the roadmap.
- Tray menus: `SetForegroundWindow(hwnd)` before `TrackPopupMenu`, `PostMessage(WM_NULL)`
  after, or the menu does not close. The send menu relies on the foreground right Explorer
  hands to the process it launches.
- Network text (device names, platforms, file names, remote errors) goes through
  `display_name`/`display_platform`/`display_file_name`/`display_text` (lib.rs) before
  any dialog, menu, tooltip or balloon: no control/bidi chars, capped length.
- `&` in menu text is an accelerator: escape device names (`&&`), done in `popup_menu`.
- Firewall: rules are per exe path. Without our rules the first `hoppyw` receive triggers
  the Windows prompt; dismissing it creates *block* rules that beat any allow rule, hence
  `apply` deletes all inbound rules for our paths first. Values never go into a UAC command
  line or script: the elevated process derives paths from its own `current_exe`.
  Environment variables do not survive UAC elevation.
- **Callback (spec §6)**: receivers (`hoppy receive`, tray) call `accept_callbacks` with
  the same dir/handler as `serve` (`lib::receive_forever` for the CLI), so the
  one-dialog guard covers both paths. Senders use `send_to_peer` (keep the `Discovery`
  alive during the send); `Progress::Fallback` drives the "trying another way" tip/line,
  `Error::Unreachable` the "Can't reach" balloon. Senders stay on an ephemeral UDP port: two
  processes sharing 47470 with SO_REUSEADDR all get multicast but unicast reaches only
  one, which would starve the tray. hoppy-core opens a listen-only 47470 socket just while
  a callback is pending. Bare-IP targets have no device id: direct only.
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
Windows-only code can be type-checked on Linux with a harness that flips `cfg(windows)` and
runs clippy against `windows-sys` (extern declarations compile anywhere); it does not link
or run anything. Manual test on a real PC: tray left/right click opens the menu at once at
the icon (check `hoppy log`: `delivery=notify`, no "waited" lines), right-click → Send with
Hoppy on 1 and 20 files (one menu), classic-menu task/toggle + Explorer restart, desktop
shortcut drop/double-click, accept dialog, balloon click, Send to menu,
send sound (silent until delivery, then the jingle once; "♪ Sound when delivered" toggle), installer music + checkbox, update flow
(balloon → download % → silent install → tray back; portable → portal), firewall task
(UAC yes/no, rules in `wf.msc`), tray firewall/Public balloons + menu item, `hoppy firewall`,
callback receive with the Hoppy rules removed (phone → PC; `hoppyw send` while the tray
runs on the same PC still receives), "trying another way" tip and "Can't reach" balloon.
