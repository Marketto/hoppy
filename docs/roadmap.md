# Roadmap

## Done — PoC (v0)
- Protocol v0 spec + shared test vectors; Rust, Kotlin and Python implementations, interop-tested.
- Windows: `hoppy` CLI; `hoppyw` tray receiver at login (no console) and "Send to → Hoppy"
  with a native device menu, progress in the tray, notifications; native accept dialog.
- Android: share target, device picker, receive service with Accept/Decline notification,
  saving to Download/Hoppy.
- Ubuntu/Linux desktop (ADR 0007): the same `hoppyw` as Windows — panel icon at login,
  Accept/Decline notifications, right-click "Send with Hoppy" (Nautilus Scripts, Nemo,
  Dolphin), device list, progress notification with Cancel, jingle, update notice;
  `Hoppy-linux-amd64.deb` for Ubuntu 22.04+.
- macOS desktop (ADR 0008): the same `hoppyw` on AppKit (objc2) — menu-bar icon at login,
  Decline-by-default accept alert, Finder service "Send with Hoppy", drops on the app icon,
  device menu at the pointer, notifications, jingle, update notice; universal
  `Hoppy-macos-universal.dmg` for macOS 12+ (Intel 2015+ and Apple silicon), ad-hoc signed.
- Five animated themes (Purikura, Showa, Manga, Yume, Night) with a first-run picker (ADR 0005).
- Release pipeline (Windows installer, Android APK, Ubuntu .deb, macOS .dmg; iOS gated) and download portal.
- Agentic setup: AGENTS.md router, skills, subagents, `tools/dev.py`, CI.

## Next — usable beta
1. Run the CI build, fix anything only a real build reveals; first device test Windows ↔ Android,
   and the Ubuntu test plan (`docs/platforms/linux.md`) on GNOME (Wayland + X11) and KDE.
2. Windows: themed picker/accept window using the theme palette; MSIX package for a
   top-level Windows 11 context-menu entry (`IExplorerCommand`) and the Share sheet target.
3. Android: transfer in a foreground service (survives leaving the screen); quick-settings tile.
4. Folders (relative paths in the offer, sanitized per segment).
5. macOS: run the manual test plan (`docs/platforms/macos-ios.md`) on an Intel 2015 Mac and an
   Apple silicon Mac; Developer ID signing + notarization (secrets only, docs/release.md);
   Finder Sync / Share extension (`.appex`) for a top-level entry and the Share menu.
6. Linux: top-level Nautilus entry (nautilus-python extension), arm64 .deb, Flatpak (needs
   the portal APIs instead of zenity/autostart files).

## v1 — secure & resumable
- Encryption + pairing (ADR to write), resume via per-chunk hashes.
- Single engine: Rust core on Android via UniFFI (ADR 0002).
- iPhone (Share extension, Wi-Fi Aware).

## v2 — beyond the LAN
- BLE discovery, Wi-Fi Aware / Direct, optional account with "my devices", remote P2P + relay.
