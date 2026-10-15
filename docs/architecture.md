# Architecture

## Goal

Two gestures to move files between any two of the user's devices: pick files, tap a
device. The app chooses the fastest available path; the user never picks a transport.

## Layers

```
┌────────────── platform shells (thin) ──────────────┐
│ Windows/Linux/macOS: tray + Send   Android: share    │
│ (crates/hoppy-cli)                 (apps/android/app)│
│ iPhone/iPad: SwiftUI + Share ext. (apps/ios) ─ C ABI │
│ (crates/hoppy-ffi → HoppyFFI.xcframework)            │
├────────────── engine (per language) ────────────────┤
│ Rust: crates/hoppy-core   Kotlin: apps/android/core  │
│   discovery · transfer · name sanitizing · limits    │
├────────────── protocol (docs/protocol.md) ──────────┤
│ v0: UDP discovery + TCP session, SHA-256 per file    │
└─────────────────────────────────────────────────────┘
```

**Why two engines in the PoC (Rust + Kotlin)?** Shipping the Rust core to Android needs
the NDK + UniFFI build chain; for a PoC a ~600-line Kotlin port was faster and is fully
interop-tested. ADR 0002 records the plan to converge on the Rust core via UniFFI for v1,
when encryption and resume make the engine too valuable to keep twice.

## Discovery (v0)

UDP port 47470: multicast `239.255.72.80` on every IPv4 interface + broadcast + optional
unicast seeds. JSON `announce`/`query`/`bye`. Receivers announce every 5 s and answer
queries; senders that do not receive use `port: 0` and an ephemeral port (spec §2 rule 6).
Nodes that cannot multicast (iOS without the entitlement) and desktop send-only processes
sweep their /24 with unicast `query` (spec §2.1, ADR 0009): every receiver answers and
records them, so no wire change was needed.

## Transfer (v0)

TCP 47471: hello → offer → human decision → (bytes + SHA-256) per file → result.
Receivers write to a pending location (`.hoppy-part` / MediaStore `IS_PENDING`) and
publish only verified files.

If the direct connect fails (receiver behind an inbound-blocking firewall), the sender
asks the receiver over UDP to connect back to an ephemeral port and runs the same session
on that socket (spec §6, ADR 0006). While that request is pending, a desktop send-only
node also listens to the multicast group on 47470 (reuse-bound, listen-only) so it can
answer the receiver's announces inside the firewall's response window.

## Transport selection (target design, v1+)

A peer carries ranked endpoints; the engine tries them in order and may upgrade mid-session:

1. Same LAN → TCP/QUIC direct (fastest).
2. Nearby, no common network → BLE discovery, then Wi-Fi Aware (Android, iOS 26) or
   Wi-Fi Direct (Android, Windows); BLE alone only for tiny payloads.
3. Remote (account, opt-in) → hole-punched P2P, else encrypted relay.
4. Unreachable → queue until reachable (own devices only).

## Security model

v1 (ADR 0010, spec §7): each device has a static X25519 key; every session runs a Noise XX
handshake (mutual authentication, forward secrecy) and carries the v0 exchange inside
ChaCha20-Poly1305. Keys are remembered on first use (TOFU) and can be verified with a
4-emoji code compared by eye («Coincide»); a changed key blocks the transfer. Old v0 apps still
work in plaintext, flagged in the UI, until the transition switch is turned off. Explicit
accept by default. Later: account-signed device keys for "my devices"; relays only ever see
ciphertext.
