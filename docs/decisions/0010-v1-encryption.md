# 0010. Protocol v1: Noise XX encryption, trust on first use, optional verification
Status: Accepted (phase 1: spec, Rust core, Python reference, CLI, FFI; phase 2: Android
core + all UIs)
Date: 2026-10-15
Supersedes: 0003 (v0 unencrypted)

## Context
v0 sends everything in clear and authenticates nothing (ADR 0003). The product owner wants
encryption always on, automatic and invisible; verifying a device must be optional at first
contact ("Verifica ora" / "Più tardi"), verified devices get a ✓, and a device seen before
that shows up with a different key must be blocked with a warning, not just asked about.
Verification: two desktops compare a short code by eye; with a phone, the phone scans a QR
code shown by the other device; the code is 4 emoji with a switch to 8 digits of the same
value. Old (v0) apps must keep working during the transition, visibly flagged.

## Decision
- **Handshake: `Noise_XX_25519_ChaChaPoly_SHA256`** via `snow` 0.10 (Rust; default
  features off: no `ring`, no BLAKE2/AES), prologue = the `HOPY 0x01` hello. XX gives mutual
  static-key authentication without knowing the peer's key in advance (needed for first
  contact and for TOFU), forward secrecy (ephemeral-ephemeral DH) and hides both identities
  from passive observers. Chosen over TLS 1.3: no certificates/X.509 parsing, one small
  dependency, a pure-Python twin is ~200 lines (stdlib only: X25519, ChaCha20-Poly1305,
  HMAC, all checked against RFC vectors), and the Kotlin core can do the same without
  Android APIs (invariant 4). The pure-Python Noise and `snow` produce byte-identical
  handshakes on the shared vectors.
- **Framing**: every Noise message is `u16 length` + bytes; after the handshake the v0
  exchange (Offer … Result) runs unchanged inside the encrypted byte stream (≤ 65 519
  plaintext bytes per message). One session format for direct and callback connections.
- **Identity**: one static X25519 key per device, created on first run, stored privately
  (desktop `identity.key` 0600; iOS `key_file`; Android app storage). Device ids stay; the
  id ↔ key binding is **trust on first use**, pinned when a transfer is accepted (not on a
  declined first contact, so a stranger's unsolicited offer cannot squat an id). States New /
  Known / Verified / KeyChanged; KeyChanged blocks (sender: error before the offer;
  receiver: `key_changed` without asking) until the user forgets the device.
- **SAS with commit/reveal**: SAS = 24 bits of SHA-256("hoppy-sas-v1" ‖ h2 ‖ n_i ‖ n_r):
  the initiator commits to its nonce in message 1 and reveals it in message 3; `h2` is the
  handshake hash after message 2. Rationale: a SAS from the transcript hash alone (as first
  sketched: HKDF(h)) lets a man in the middle grind its own later messages offline until
  both screens show the same 24 bits (~2²⁴ cheap tries, seconds on a laptop); with the
  commitment every attempt succeeds with probability 2⁻²⁴ (ZRTP / Bluetooth SSP style).
  A plain SHA-256 with a label replaces HKDF (one primitive fewer for every implementation;
  same security as a random oracle on a 32-byte secret-dependent input).
- **Code display**: 4 emoji (6 bits each) from a curated table of 64 (Unicode ≤ 9 single
  code points, distinct silhouettes, names in Italian and English) or the same value as
  8 zero-padded digits. The table lives in `docs/protocol.md` §7.6 and
  `docs/test-vectors.json`; implementations copy it and test against the vectors.
- **QR verification**: `hoppy:verify:1:<id>:<fingerprint>:<one-time code>`. The scanner
  checks the fingerprint against the key it knows, marks it verified, then proves the scan to
  the shower in a new v1 session that must present exactly that key (`{"verify":code}`);
  the shower marks the scanner verified only for a live code, so a desktop shows ✓ after a
  phone scanned it, and nobody can claim to be verified without seeing the screen. The code
  is never sent over a session with another key.
- **Negotiation**: `"crypto":1` is added to every v1 datagram and callback hello (additive,
  `hoppy` stays 0, so v0 apps still see v1 devices). Senders use v1 when the peer
  advertised it, when its key is remembered (no downgrade), or when nothing is known (typed
  address: falls back to v0 only if the peer closes right after the v1 hello). Receivers
  accept both; plaintext from an id with a remembered key is refused
  (`encryption_required`). UIs flag every plaintext session. `ALLOW_PLAINTEXT_DEFAULT`
  (Rust `transfer.rs`) turns the transition off in a later release.
- **Core API for UIs** (`Local` = device + key + trust store + policy): senders get
  `Progress::Session(SessionInfo)` (encrypted, trust state, peer key, SAS) before the
  offer; `ReceiveHandler::decide` receives the same `SessionInfo`; `blocked` and `verified`
  hooks; `Trust::{mark_verified, forget, retrust, begin_qr, end_qr, check_qr}`,
  `transfer::confirm_qr`. The FFI exposes them as JSON (`crates/hoppy-ffi`).

## Consequences
- Every v1 ↔ v1 session is encrypted and authenticated; the 30-byte overhead per 64 KiB is
  negligible (ChaCha20-Poly1305 in Rust runs at GB/s); one extra round trip for the
  handshake (message 3 and the offer leave together).
- First contacts remain TOFU: protection against an attacker present at the very first
  contact needs the optional code/QR check. Discovery stays unauthenticated (spoofing only
  redirects to an impostor that then shows as New or is blocked as KeyChanged).
- Until Kotlin implements v1 (phase 2), Android ↔ desktop/iOS transfers run in plaintext,
  flagged; the interop test keeps checking that fallback.
- A reinstalled device has a new key: its peers see KeyChanged and must forget it — the
  UI must make "Dimentica dispositivo" obvious in the warning.
- ADR 0003's "no public release before v1 encryption" is met for v1 ↔ v1 only; remote/relay
  transports still wait until plaintext is switched off.
