---
name: hoppy-windows
description: Use for the Windows experience - the "Send to → Hoppy" entry, the hoppy CLI commands and console UX, installer script, firewall, startup receiver, and the future native Windows UI/shell extension.
---

# Windows

PoC UX: Explorer → right click → **Send to → Hoppy** → a console window lists nearby
devices → type a number → progress bar → "Delivered". Receiving runs `hoppy receive`
minimized at login and asks with a native Yes/No dialog (`MessageBoxW`).

## Pieces

- `crates/hoppy-cli/src/main.rs` — commands `send`, `receive`, `peers`, `name`, `install`, `uninstall`.
  `send --pause` is what the Send To shortcut runs (keeps the console open at the end).
- `crates/hoppy-cli/src/platform.rs` — `install()` writes two `.lnk` via PowerShell
  `WScript.Shell` (values passed through env vars, never string-interpolated):
  `%APPDATA%\Microsoft\Windows\SendTo\Hoppy.lnk` and `...\Startup\Hoppy receiver.lnk`.
- `apps/windows/install.ps1` — copies `hoppy.exe` to `%LOCALAPPDATA%\Programs\Hoppy` and runs `hoppy install`.
- Docs: `docs/platforms/windows.md` (Windows 11 menu, firewall, roadmap to a real shell extension).

## Gotchas

- Windows 11 shows "Send to" under "Show more options"; a top-level entry needs an
  `IExplorerCommand` handler in a signed MSIX package (planned, see roadmap).
- First `receive` triggers the firewall prompt; the user must allow **Private** networks.
- Open files cannot be renamed/deleted: close handles first (see `PartFile`).
- Paths with emoji/non-ASCII: always `PathBuf`/`OsStr`, never `String` round-trips.

## Verify

Linux can't build the Windows target here. Run `python3 tools/dev.py check`, then rely on
the `windows-latest` CI job (`cargo build` + `cargo test` + artifact `hoppy.exe`).
Manual test: `hoppy receive --yes` on one PC, Send To on another.
