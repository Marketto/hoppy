# 0002. Rust engine on desktop, Kotlin port on Android for the PoC
Status: Accepted (revisit at v1)
Date: 2026-10-14

## Context
The target design is one Rust core on every platform (UniFFI bindings). The PoC
environment had no Android NDK, and the PoC needed to reach a phone fast.

## Decision
Desktop uses `crates/hoppy-core`. Android uses `apps/android/core`, a pure-JVM Kotlin
port of the same protocol. A Python reference (`tools/hoppy_ref.py`) is the tie-breaker.
All three are bound by `docs/protocol.md`, `docs/test-vectors.json` and `tools/interop.sh`.

## Consequences
Every wire change costs three edits (skill `hoppy-protocol-change`). At v1 (encryption,
resume) switch Android to the Rust core via UniFFI and keep Kotlin only as UI.
