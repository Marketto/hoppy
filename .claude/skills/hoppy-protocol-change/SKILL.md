---
name: hoppy-protocol-change
description: Use when changing anything on the Hoppy wire (discovery datagrams, TCP framing, offer/decision/result JSON, limits, ports, file-name sanitizing). Keeps Rust, Kotlin and Python implementations, the spec and the test vectors in lockstep.
---

# Changing the Hoppy protocol

The protocol has **three implementations** that must agree byte for byte:

| Layer | Rust | Kotlin (Android) | Python reference |
| --- | --- | --- | --- |
| constants, frames, JSON types | `crates/hoppy-core/src/protocol.rs` | `apps/android/core/src/main/kotlin/app/hoppy/core/Protocol.kt` | `tools/hoppy_ref.py` (top + framing) |
| discovery | `crates/hoppy-core/src/discovery.rs` | `.../core/Discovery.kt` | `hoppy_ref.py` `announcer` / `peers` |
| transfer | `crates/hoppy-core/src/transfer.rs` | `.../core/Transfer.kt` | `hoppy_ref.py` `send` / `handle` |
| names (§4) | `crates/hoppy-core/src/sanitize.rs` | `.../core/FileNames.kt` | `hoppy_ref.py` `sanitize` |

## Steps

1. **Spec first.** Edit `docs/protocol.md`. Decide compatibility:
   - Additive JSON field that old peers may ignore → no version bump (spec §2 rule 5).
   - Anything else (new frame, new byte, changed meaning) → bump `VERSION` everywhere,
     add a "v_n" section, keep the old one until the ADR says otherwise.
2. **Vectors.** Add cases to `docs/test-vectors.json` that exercise the change (every
   implementation's tests read this file). For byte layouts, update the §5 example via
   `python3 tools/hoppy_ref.py vectors`.
3. **Implement in all three**, same names where possible. Limits (frame size, file count,
   timeouts) must use the same values.
4. **Tests**: Rust unit + `crates/hoppy-core/tests/end_to_end.rs`; Kotlin
   `apps/android/core/src/test/kotlin/app/hoppy/core/CoreTest.kt`; Python `selftest`.
5. **Interop**: `python3 tools/dev.py interop` (runs Rust↔Python and, with Gradle, Kotlin).
6. **Review**: run the `protocol-reviewer` subagent on the diff.
7. If the change is a decision (new transport, crypto, version policy) write an ADR
   (`hoppy-docs-adr` skill).

## Security checklist (every change)

- Lengths are bounded before allocating (`MAX_JSON_FRAME`, sizes are u64/Long, no negative).
- Received names are sanitized; nothing is written outside the target directory.
- Partial data is never visible as a finished file; it is deleted on any error.
- Timeouts on every blocking read; a silent peer cannot pin a thread forever.
- Unknown/foreign-version discovery messages are ignored, never crash.

## v1 encryption (§7, ADR 0010)

- Session crypto, SAS (4 emoji only: no digits, no QR — ADR 0010 amendment) and the TOFU trust store live in
  `crates/hoppy-core/src/{crypto,session,trust}.rs`, `tools/hoppy_ref.py` (pure-Python X25519/ChaCha20-Poly1305/Noise XX)
  and the Kotlin core (`Crypto.kt`, `Noise.kt`, `Trust.kt`, `Transfer.kt`; tested by `V1Test`); all three must match
  the `v1` block of `docs/test-vectors.json` byte for byte.
- Security checklist: keep the SAS commit/reveal, never fall back to plaintext for a peer that advertised
  `crypto` or whose key is remembered, KeyChanged always blocks, the emoji table is fixed (64 entries).
