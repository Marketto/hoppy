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
- iPhone/iPad (ADR 0009): SwiftUI app + Share extension over the Rust core
  (`crates/hoppy-ffi`), unicast discovery sweep (no multicast entitlement), receive while
  open with Decline-by-default alert, Files integration; unsigned `Hoppy-ios-unsigned.ipa`
  sideloaded with Sideloadly (free Apple ID, 7 days), iOS 15+.
- Five animated themes (Purikura, Showa, Manga, Yume, Night) with a first-run picker (ADR 0005).
- Release pipeline (Windows installer, Android APK, Ubuntu .deb, macOS .dmg, iOS unsigned .ipa) and download portal.
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
7. iPhone/iPad: run the manual test plan (`docs/platforms/ios.md`) on the iPad Air; theme
   picker (five themes) and idle music; with a Developer account: TestFlight, multicast
   entitlement (sweep off), App Group (shared name for the Share extension).

## v1 — secure & resumable
- Encryption + device verification (ADR 0010): phase 1 done — spec §7, Rust core, Python
  reference, vectors, CLI (`hoppy trust`), FFI. Phase 2: Android (done: Kotlin core v1,
  interop-tested; code panel, QR scan/show, ✓, KeyChanged dialog; awaits the CI build and a
  device test), desktop dialogs (emoji/number switch, "Coincide", QR display),
  iOS UI (done: persistent key/trust per container, code + «Coincide», QR scan/show,
  KeyChanged alert; awaits the CI job `ios` and the manual plan); then switch plaintext off.
- iPhone/iPad updates at zero cost: SideStore source `sidestore.json` on the portal + daily
  in-app notice (ADR 0009 addendum).
- Resume via per-chunk hashes.
- Single engine: Rust core on Android via UniFFI (ADR 0002); iOS already uses it (ADR 0009).
- iPhone/iPad: Wi-Fi Aware (iOS 26) when no shared network.

## v2 — beyond the LAN
- BLE discovery, Wi-Fi Aware / Direct, optional account with "my devices", remote P2P + relay.
