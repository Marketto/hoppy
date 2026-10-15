# 0010. Protocol v1: Noise XX encryption, trust on first use, optional verification
Status: Accepted (phase 1: spec, Rust core, Python reference, CLI, FFI; phase 2: Android
core + all UIs); amended 2026-10-15: verification is 4 emoji only (QR and digits removed, below)
Date: 2026-10-15
Supersedes: 0003 (v0 unencrypted)

## Context
v0 sends everything in clear and authenticates nothing (ADR 0003). The product owner wants
encryption always on, automatic and invisible; verifying a device must be optional at first
contact ("Verifica ora" / "Più tardi"), verified devices get a ✓, and a device seen before
that shows up with a different key must be blocked with a warning, not just asked about.
Verification (as amended): both users compare 4 emoji by eye and press «Coincide» or
«Più tardi». Old (v0) apps must keep working during the transition, visibly flagged.

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
  code points, distinct silhouettes, names in Italian and English); the only form (the
  8-digit alternative was removed, see the amendment). The table lives in `docs/protocol.md` §7.6 and
  `docs/test-vectors.json`; implementations copy it and test against the vectors.
- ~~**QR verification**~~ (`hoppy:verify:1:…` payload, one-time codes, `{"verify":code}`
  first frame): removed by the amendment below.
- **Negotiation**: `"crypto":1` is added to every v1 datagram and callback hello (additive,
  `hoppy` stays 0, so v0 apps still see v1 devices). Senders use v1 when the peer
  advertised it, when its key is remembered (no downgrade), or when nothing is known (typed
  address: falls back to v0 only if the peer closes right after the v1 hello). Receivers
  accept both; plaintext from an id with a remembered key is refused
  (`encryption_required`). UIs flag every plaintext session. `ALLOW_PLAINTEXT_DEFAULT`
  (Rust `transfer.rs`) turns the transition off in a later release.
- **Core API for UIs** (`Local` = device + key + trust store + policy): senders get
  `Progress::Session(SessionInfo)` (encrypted, trust state, peer key, SAS) before the
  offer; `ReceiveHandler::decide` receives the same `SessionInfo`; `blocked` hook;
  `Trust::{mark_verified, forget, retrust}` (as amended). The FFI exposes them as JSON (`crates/hoppy-ffi`).

## Consequences
- Every v1 ↔ v1 session is encrypted and authenticated; the 30-byte overhead per 64 KiB is
  negligible (ChaCha20-Poly1305 in Rust runs at GB/s); one extra round trip for the
  handshake (message 3 and the offer leave together).
- First contacts remain TOFU: protection against an attacker present at the very first
  contact needs the optional emoji check. Discovery stays unauthenticated (spoofing only
  redirects to an impostor that then shows as New or is blocked as KeyChanged).
- Android implements v1 in its pure-JVM core (phase 2, below); the interop test checks Kotlin
  against Rust and Python encrypted, and its plaintext fallback against a v0 node.
- A reinstalled device has a new key: its peers see KeyChanged and must forget it — the
  UI must make "Dimentica dispositivo" obvious in the warning.
- ADR 0003's "no public release before v1 encryption" is met for v1 ↔ v1 only; remote/relay
  transports still wait until plaintext is switched off.

## Hardening after security review (2026-10-15)
- **Online code guessing.** The commit/reveal stops offline grinding, but the party sending
  message 3 learns the code first: a man in the middle posing as the sender can abort and
  retry against the receiver until its code matches the one fixed with the real sender. A
  responder-side commitment does not help (the last revealer always learns first, and the
  receiver only shows the code with an offer). Fix: receivers count sessions that end after
  message 2 without a first frame — at most 5 per IP and 20 in total per minute, per-IP
  backoff 60 s doubling to 1 h, refusal = empty frame `00 00` ("busy", never taken for a v0
  node), user warning; senders keep one 130 s deadline for handshake + Decision and empty
  transport messages are forbidden (they could stall a reader forever). Bound: ≤ 61 attempts
  per intercepted send → P ≤ 61/2²⁴ ≈ 3.6 × 10⁻⁶, each failure visible. Trade-off: an active
  attacker can make a receiver answer "busy" for a while.
- Low-order X25519 keys and short handshake messages are rejected; handshake ids/names are
  cleaned before use; `crypto` is parsed leniently.
- Files: Windows keeps `device.json`, `identity.key` and `trust.json` in local AppData with an
  owner-only ACL (roaming profiles would copy the key to a server and give two PCs one
  identity); the key is created race-free and never overwritten if its version is unknown;
  the trust store is merged under a lock file and a damaged one is set aside, not fatal; save
  errors reach the user; the private key is wiped from memory on drop.
- TOFU only on a person's accept (not `--yes`); New devices reusing a known name are flagged;
  typed addresses where a remembered device encrypted never fall back to plaintext; the
  specific reasons `key_changed` / plaintext-from-known stay local (wire: `declined`), and the
  matching warnings are rate-limited.

## iPhone/iPad (phase 2, 2026-10-15)
- The app and the Share extension pass `key_file` / `trust_file` (Application Support of each
  container, excluded from backups); the device id is re-generated whenever the key is new, so
  a reinstall or restore appears as a New device, not KeyChanged. The extension cannot share the
  app's key with a free signature (no App Group / keychain sharing, ADR 0009 addendum): it is a
  separate device, «iPad (Condividi)», with its own key and trust store.
- UI as on the other platforms: rows 🔒 / ✓ / ⚠ non cifrato; the code (4 emoji, «123» → 8 digits,
  VoiceOver reads the names) with «Coincide», «Verifica con QR» (AVFoundation, falls back to the
  code without camera permission), «Mostra il mio QR» (CoreImage; receiving stays on while it is
  shown), «Più tardi»; KeyChanged → blocking alert with «Dimentica dispositivo».

## Android (phase 2, 2026-10-15)
- The Kotlin `:core` implements §7 itself (still pure JVM, invariant 4): Noise XX, SAS
  commit/reveal, framing, trust store (same JSON as desktop), aborted-handshake guard, busy
  frame, deadlines. X25519 and ChaCha20-Poly1305 come from the BouncyCastle lightweight API
  (`bcprov-jdk18on`, pure Java, constant time): Android 10 has no X25519 in JCA, and
  hand-written field arithmetic is not worth the risk. SHA-256/HMAC come from the JDK. It
  passes every `v1` vector (positive and negative) and the RFC KATs, and the interop legs
  K1–K10 (K1–K9 after the amendment). Cost: the APK grows by the (unshrunk) BouncyCastle classes until R8 is enabled.
- Key and trust store in app-private `noBackupFilesDir` (excluded from cloud backup and
  device-to-device transfer); a new key comes with a new device id (as on iOS).
- UI: device rows 🔒 / ✓ / ⚠ non cifrato; while sending to a New/Known device the code shows
  over the list (4 emoji, «123» → 8 digits, TalkBack reads the names) with «Coincide»,
  «Verifica con QR», «Mostra il mio QR», «Più tardi»; the incoming-offer notification carries
  the code and a «Verifica» action (Accept/Decline unchanged, default decline); KeyChanged →
  blocking dialog / notification with «Dimentica dispositivo»; code-guessing and refused
  senders → "Security warnings" notifications. QR: ZXing Android Embedded (scan, camera
  permission asked on use, falls back to the code) and ZXing encoding for «Mostra il mio QR»
  (receiving stays on while it is shown). A scanned device never seen before is verified only
  through a session presenting exactly the scanned key.

## Desktop: Windows, Linux, macOS (phase 2, 2026-10-15)
- One description of every dialog (`crates/hoppy-cli/src/verify.rs`, `gui.rs`), drawn by
  each backend: a small Win32 window of Hoppy's own (MessageBox cannot show large emoji, a
  toggle or a QR code; GDI draws emoji in one colour, so each has its name under it), an
  `NSAlert` accessory view on macOS, zenity with Pango markup on Linux (QR code as Unicode
  half blocks, black on white). The QR code is generated with the `qrcode` crate (no
  renderer features) as a module matrix.
- Receiver (tray): accept dialog with the code (4 emoji + names, «123» → 8 digits) for
  New/Known senders; Rifiuta (default, also on close/timeout) / Accetta / «Coincide,
  accetta» (accept + mark verified). From a phone it also shows this device's QR code
  (`Local::begin_qr`, `end_qr` when the dialog closes); the `verified` hook (matched by key
  fingerprint) replaces it with ✓. Linux shows the accept question as a notification first
  (Accept / Decline / «Verifica…»).
- Sender (`hoppyw send`): device menu 🔒 / ✓ / ⚠ non cifrato; a code window with «Coincide»
  while the receiver decides, closed by the send worker when the transfer ends. No QR code
  there: the one-time code would live in the short-lived send process while the phone's
  confirmation reaches the tray receiver.
- KeyChanged on either side: blocking warning with «Dimentica dispositivo» (never the
  default); tray menu «Dispositivi fidati…» lists and forgets remembered devices;
  code-guessing warnings at most once a minute; a damaged trust store is announced.

## Amendment: 4 emoji only, QR and digits removed (2026-10-15)
- **Decision (product owner):** device verification is only the 4 emoji, compared by eye, with
  «Coincide» (mark the other device verified on this device) or «Più tardi». Removed from the
  product: the 8-digit form of the code («123»), and the whole QR verification (payload
  `hoppy:verify:1:…`, one-time codes, the `{"verify":code}` first frame and its replies,
  the remote `verified` notification, «Verifica con QR» / «Mostra il mio QR»).
- **Why:** one way to verify is simpler to explain and to get right; less attack surface (no
  second kind of first frame, no one-time-code state in the receiver, no QR parser, no camera
  input); the camera permission and the QR dependencies (ZXing on Android, AVFoundation /
  CoreImage on iOS, `qrcode` on desktop) are gone.
- **Unchanged:** the 24-bit SAS value, its derivation with commit/reveal, the emoji table and
  the aborted-handshake limits. «Coincide» is local: nothing is sent, so each user confirms on
  their own device.
- **Wire:** the first encrypted frame is always an Offer; anything else (including a
  `{"verify":…}` from a development build) is answered `invalid_offer` like any malformed offer
  (spec §7.4). Spec §7.7 (QR) is gone and the limits section is now §7.7. Vectors: `qr_example`
  and every `digits` field removed.
- **APIs removed:** Rust `QrPayload`, `crypto::{QR_PREFIX, QR_CODE_HEX_LEN, new_qr_code}`,
  `Sas::digits`, `Trust::{begin_qr, end_qr, check_qr}`, `trust::QR_CODE_TTL`,
  `Local::begin_qr`, `transfer::confirm_qr`, `ReceiveHandler::verified`,
  `Error::VerificationFailed`; FFI `hoppy_node_qr_begin/qr_end/qr_scan`, events `qr_done` and
  `verified`, `sas.digits`; Kotlin `QrPayload`, `QrVerification`, `QrResult`,
  `Crypto.{QR_PREFIX, QR_CODE_HEX_LEN, newQrCode}`, `Sas.digits`,
  `Trust.{beginQr, endQr, takeQrCode, checkQr}`, `Local.beginQr`, `TransferClient.confirmQr`,
  `ReceiveHandler.onVerified`, `VerificationFailedException`, `UnknownDeviceException`;
  Python `verify` command and `--show-qr`; CLI `hoppy receive --show-qr` and
  `hoppy trust verify --qr/--to` (`hoppy trust verify DEVICE` stays: "They match" after
  comparing the emoji). The QR and «123» parts of the phase-2 sections above are superseded.
