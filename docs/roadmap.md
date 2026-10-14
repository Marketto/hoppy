# Roadmap

## Done — PoC (v0)
- Protocol v0 spec + shared test vectors; Rust, Kotlin and Python implementations, interop-tested.
- Windows: `hoppy` CLI, "Send to → Hoppy", startup receiver, native accept dialog.
- Android: share target, device picker, receive service with Accept/Decline notification,
  saving to Download/Hoppy, Kawaii and Pro themes.
- Agentic setup: AGENTS.md router, skills, subagents, `tools/dev.py`, CI.

## Next — usable beta
1. Run the CI build, fix anything only a real build reveals; first device test Windows ↔ Android.
2. Windows: small native window for the device picker (instead of the console) using the
   theme palette; top-level Windows 11 context-menu entry (`IExplorerCommand`, MSIX).
3. Android: transfer in a foreground service (survives leaving the screen); quick-settings tile.
4. Folders (relative paths in the offer, sanitized per segment).

## v1 — secure & resumable
- Encryption + pairing (ADR to write), resume via per-chunk hashes.
- Single engine: Rust core on Android via UniFFI (ADR 0002).
- macOS (Share extension, menu bar), Linux (Nautilus/Dolphin actions), iPhone (Share
  extension, Wi-Fi Aware).

## v2 — beyond the LAN
- BLE discovery, Wi-Fi Aware / Direct, optional account with "my devices", remote P2P + relay.
