---
name: hoppy-rust-core
description: Use when working on the Rust crates (hoppy-core library or the hoppy CLI) - discovery, transfer, sanitizing, errors, tests, performance. Covers layout, conventions and how to verify.
---

# Rust core

`crates/hoppy-core` is the desktop engine; `crates/hoppy-cli` holds the `hoppy` CLI and the
`hoppyw` Windows tray/GUI binary, sharing `src/lib.rs`.
Design notes: `docs/architecture.md`. Wire rules: `docs/protocol.md`.

## Layout

- `protocol.rs` — constants, serde types, `write_json`/`read_json`, hello. Pure, unit-tested.
- `sanitize.rs` — spec §4; tests read `docs/test-vectors.json` via `include_str!`.
- `transfer.rs` — `send_files` (client) and `serve`/`receive_one` (server, one thread per
  connection). `ReceiveHandler` is the UI hook (decide/progress/finished). `PartFile`
  closes the handle **before** rename/delete because Windows forbids both on open files.
- `discovery.rs` — `Discovery::start(DiscoveryConfig)`; joins multicast on every IPv4
  interface and sends per interface (multi-adapter Windows PCs). `bind_port: 0` +
  `transfer_port: 0` = send-only node (spec §2 rule 6). Tests use `seeds` + `use_multicast: false`.

## Conventions

- Blocking std I/O + threads; no tokio in v0 (keeps the binary small and Windows-friendly).
- No `unwrap()` on network or file data; map to `Error::{Io,Protocol,Input,Remote}`.
- `#[cfg(windows)]` code lives only in `crates/hoppy-cli/src/platform.rs`. It cannot be
  compiled on Linux without the Windows target, so keep it tiny and let CI (windows-latest)
  check it. Its non-Windows stub (same API, `Tray::new` fails) keeps `gui.rs` compiled and
  linted on Linux; logic that can be pure (string table, buffer parsing) is unit-tested there.
- Every new behaviour gets a test in `tests/end_to_end.rs` (loopback, temp dirs).

## Verify

```
python3 tools/dev.py check     # fmt, clippy -D warnings, tests, docs lint
python3 tools/dev.py interop   # if bytes on the wire could change
```
