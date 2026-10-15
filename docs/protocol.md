# Hoppy wire protocol — v0 + v1

Status: **PoC**. Implemented in three places that MUST stay in lockstep:

| Implementation | Path |
| --- | --- |
| Rust core (reference for desktop and iOS) | `crates/hoppy-core/src/protocol.rs`, `discovery.rs`, `transfer.rs`; v1: `crypto.rs`, `session.rs`, `trust.rs` |
| Android (Kotlin) | `apps/android/core/src/main/kotlin/app/hoppy/core/` (v0 only until phase 2 of ADR 0010) |
| Python reference / test tool | `tools/hoppy_ref.py` |

Any change follows the `hoppy-protocol-change` skill. Sections 1–6 are **v0**: no
encryption, no authentication (ADR 0003). Section 7 is **v1** (ADR 0010): every session
between two v1 devices is encrypted and authenticated with Noise XX, devices are remembered
by key (trust on first use) and can be verified with a short code or a QR code. v1 keeps the
v0 framing inside the encrypted channel and still talks to v0 devices, flagged as not
encrypted, during the transition (§7.1).

## 1. Constants

| Name | Value |
| --- | --- |
| Discovery UDP port | `47470` |
| Default transfer TCP port | `47471` (if busy, use any port and announce it) |
| Multicast group (IPv4) | `239.255.72.80` |
| Magic | ASCII `HOPY` = `48 4F 50 59` |
| Callback magic (§6) | ASCII `HOPC` = `48 4F 50 43` |
| Protocol version byte | `0x00` (v0 hello, `hoppy` field of every datagram); `0x01` = v1 session hello (§7.2) |
| `crypto` field (v1 datagrams, callback hello) | `1` (§7.1) |
| Noise protocol (v1) | `Noise_XX_25519_ChaChaPoly_SHA256`, prologue `48 4F 50 59 01` |
| Max Noise message (v1) | 65 535 bytes (`u16` length); handshake messages ≤ 2 048 bytes |
| QR one-time code lifetime (v1) | 10 min while the code is on screen |
| Max JSON frame | 1 MiB (1 048 576 bytes) |
| Max files per offer | 10 000 |
| Announce interval | 5 s |
| Peer expiry | 15 s without any message |
| Direct connect timeout before callback (§6) | 3 s |
| Callback window / re-send / connect timeout (§6) | 20 s / 2 s / 3 s |
| Callback hello: whole-read deadline / max JSON frame (§6) | 3 s / 1 KiB (1 024 bytes) |
| Callbacks per receiver | ≤ 10 per minute per source IP, ≤ 30 per minute in total, 1 outstanding per source IP |

## 2. Discovery (UDP)

Every message is one UTF-8 JSON object in one datagram (≤ 1024 bytes).

```json
{"hoppy":0,"type":"announce","id":"5f0c…","name":"Marco-PC","platform":"windows","port":47471}
```

| Field | Type | Notes |
| --- | --- | --- |
| `hoppy` | int | protocol version, `0` |
| `type` | string | `announce`, `query`, `bye` or `callback` (§6) |
| `id` | string | stable device id (UUID v4), persisted on the device |
| `name` | string | human readable, ≤ 64 chars |
| `platform` | string | `windows`, `android`, `linux`, `macos`, `ios` |
| `port` | int | TCP transfer port of the sender (`callback`: its callback listener) |
| `token`, `to` | string | `callback` only, see §6 |
| `crypto` | int | v1 nodes only: `1` = speaks encrypted sessions (§7.1). Absent on v0 nodes |

Rules:

1. The peer address is the **source IP** of the datagram plus `port`.
2. On start a node sends `query` then `announce`; afterwards `announce` every 5 s.
   Targets: multicast `239.255.72.80:47470` on every non-loopback IPv4 interface,
   plus `255.255.255.255:47470` (fallback for networks that drop multicast),
   plus any explicit seed address configured by the user. Nodes that cannot multicast
   use the unicast sweep instead (§2.1).
3. On `query` the receiver records the sender as a peer **and** replies with an
   `announce` sent by unicast to the datagram's source address and port.
4. On `bye` the peer is removed. Messages carrying the node's own `id` are ignored.
5. Unknown fields MUST be ignored; messages with an unknown `type` or a different
   `hoppy` version MUST be ignored (forward compatibility).
6. `port: 0` marks a **send-only** node (e.g. the desktop "Send to" process while a
   separate receiver runs). It sends `query` every 5 s instead of `announce`,
   binds an ephemeral UDP port so unicast replies reach it, is never listed as a
   peer, and does not answer queries. It need not send `bye` (the Rust core does not:
   a desktop sender shares its `id` with the tray receiver, which a `bye` would remove).

### 2.1 Unicast sweep (nodes without multicast)

Client behaviour only: no new message, field or value; every v0 node already supports it
through rules 1–3. A node that cannot send or hear multicast/broadcast (iOS apps without
Apple's multicast entitlement, ADR 0009) MAY discover by unicast:

1. **Sweep**: send `query` (its current `port`) to `47470` of every host of each local IPv4
   subnet with a private RFC 1918 address (10/8, 172.16/12, 192.168/16), limited to the /24
   around its own address (a wider subnet is narrowed to that /24; /31 and /32 are skipped),
   skipping its own address and the network/broadcast addresses, only on interfaces that
   are up and broadcast-capable (no cellular, no point-to-point VPN, no obvious virtual
   adapters such as docker0, virbr0, vboxnet, Hyper-V vEthernet), preferring the one that
   carries the default route. At most 256 targets (one /24), paced (≈ 2 ms apart). At start, then every 10 s (shorter than the 15 s peer expiry, so nodes that
   only hear it through the sweep keep listing it), and on user refresh (at most every 3 s).
   Seeds (rule 2) still get every message.
2. Receivers answer each `query` with a unicast `announce` and record the sweeping node as a
   peer when its `port != 0` (rule 3): both sides learn each other.
3. While receiving (`port != 0`) it also sends `query` to each known peer every 5 s, and a
   node that does not multicast sends `bye`/`announce` for state changes to its known peers
   by unicast too (e.g. `bye` when it stops receiving or quits).
4. A send-only node on an ephemeral port (rule 6) never hears these nodes' unicast
   `announce`, so desktop send-only processes without explicit seeds sweep as well
   (opt out: `hoppy --no-sweep`, or `HOPPY_NO_SWEEP=1` for `hoppyw` too).
5. Unicast to known peers goes only to local addresses (spec §6 ranges).

Implementation: `Sweep` in `crates/hoppy-core/src/discovery.rs` (`sweep_targets`, tested);
Kotlin and Python do not sweep (Android's node is bound to 47470 and hears sweep queries).

## 3. Transfer (TCP)

All integers are unsigned big-endian. A *JSON frame* is `u32 length` + `length` bytes of
UTF-8 JSON (length ≤ 1 MiB).

```
sender → receiver   "HOPY" 0x00                          5 bytes hello
sender → receiver   JSON frame: Offer
receiver → sender   JSON frame: Decision
-- only if accept == true, for each file in offer order --
sender → receiver   exactly `size` bytes of file content
sender → receiver   32 bytes SHA-256 of that content
-- end loop --
receiver → sender   JSON frame: Result
```

**Offer**

```json
{"sender":{"id":"5f0c…","name":"Marco-PC","platform":"windows"},
 "files":[{"name":"foto.jpg","size":2048}]}
```

**Decision**: `{"accept":true}` or `{"accept":false,"reason":"declined"}`.
Reasons: `declined`, `busy`, `invalid_offer`, `too_many_files`.

**Result**: `{"ok":true}` or `{"ok":false,"error":"hash mismatch: foto.jpg"}`.

The receiver closes the connection after the Result. Either side may close at any
time; a partial file MUST then be deleted.

## 4. File names (receiver side, mandatory)

The receiver never trusts `name`. Algorithm (identical in all implementations,
test cases in `docs/test-vectors.json`):

1. Replace `\` with `/` and keep only the part after the last `/`.
2. Remove control characters (U+0000–U+001F, U+007F).
3. Replace each of `< > : " | ? *` with `_`.
4. Trim spaces and dots at both ends.
5. If empty, `.` or `..` → the offer is invalid (`invalid_offer`).
6. If the stem (before the first `.`) is a Windows reserved name
   (`CON PRN AUX NUL COM1-9 LPT1-9`, case-insensitive) → prefix `_`.
7. Truncate to 200 characters, keeping the extension (text after the last `.`) when
   it is ≤ 20 characters.

Content is written to a temporary `.hoppy-part` file and renamed only after the hash
matches. Name clashes become `name (1).ext`, `name (2).ext`, ….

## 5. Byte-level example

Offer for one file `a.txt` of 5 bytes `hello`, using `offer_json` from
`docs/test-vectors.json` (regenerate with `python3 tools/hoppy_ref.py vectors`):

```
48 4F 50 59 00                     hello
00 00 00 7D {"sender":…}           offer frame (125 bytes of JSON)
68 65 6C 6C 6F                     content
2C F2 4D BA … 93 8B 98 24          SHA-256("hello")
```

## 6. Callback (reverse connect)

For receivers that cannot accept inbound TCP (e.g. a managed Windows PC whose firewall
blocks inbound connections and whose user cannot add rules) but can connect out. Additive
to v0: no version bump; older nodes ignore the unknown `callback` type (§2 rule 5; true for
every v0 implementation, which reject unknown `type` values) and simply stay unreachable.

**Why it works.** Stateful firewalls allow outbound TCP, and Windows by default accepts a
unicast UDP reply to its own multicast/broadcast for about 3 s ("allow unicast response").
The receiver multicasts an `announce` every 5 s from its discovery port, so a sender that
answers each announce immediately gets its datagram through.

**Datagram** (sender → receiver discovery address, from the sender's discovery socket):

```json
{"hoppy":0,"type":"callback","id":"<sender id>","name":"Phone","platform":"android",
 "port":50123,"token":"00112233445566778899aabbccddeeff","to":"<receiver id>"}
```

`port` is the sender's ephemeral TCP callback listener (never 0). `token` is 128 random
bits from a CSPRNG as exactly 32 lowercase hex characters. `to` is the receiver's `id`.
A `callback` with a missing/invalid `token`, empty `to` or `port` 0 is invalid and ignored.
A `callback` never adds or refreshes a peer.

**Sender** (when TCP to the receiver's `addr:port` fails within 3 s — timeout, refused,
no route — and it runs discovery):

1. Listen on an ephemeral TCP port, create a fresh token. The receiver's **known IP** is
   the IP it was discovered at (the one the failed direct connect went to).
2. Send the datagram to the receiver's discovery address (UDP source of its last
   announce/query; else its known IP + 47470) right away, again every 2 s, **and
   immediately in reply to every `announce`/`query` from that receiver** (lands inside the
   firewall's response window). Use the discovery socket so source port = the one the
   receiver's firewall already saw. Stop after 20 s → "unreachable".
   While the request is pending, an `announce`/`query` carrying the receiver's `id` counts
   as "from that receiver" only if its UDP source IP equals the known IP and is local
   (receiver step 1 ranges); any other such datagram is ignored entirely: it is not
   answered, does not move the re-send target and does not update the peer. (Otherwise
   anyone on the LAN could claim the receiver's `id` and collect the token.)
3. Close every accepted connection whose remote IP is not the known IP, before reading
   anything. On the others, read the callback hello with **one overall 3 s deadline** for
   the whole hello (not per read) and a JSON frame of at most **1 KiB**; a frame that is
   larger, not a JSON object, or late closes the connection. Accept it only if the token
   matches (constant-time compare) and `id` equals the receiver's id; otherwise close it
   and keep waiting. Use the first valid one, close the listener.
4. Continue on that socket **exactly as §3 from the sender's side** (`HOPY 0x00`, Offer,
   …). The sender stays the client of the session even though it accepted the TCP connection.

**Receiver** (only while it is a receiving node, `port != 0`):

1. Ignore the datagram unless `to` equals its own id and the UDP source IP is loopback
   (127/8, ::1), private (10/8, 172.16/12, 192.168/16), CGNAT (100.64/10), link-local
   (169.254/16, fe80::/10) or IPv6 ULA (fc00::/7).
2. Limits: ignore it while a callback from the same source IP is still running; ignore a
   token that already led to a successful connect-back in the last 2 min (re-sends); at
   most 10 attempts started per minute per source IP and 30 per minute in total. A token
   is recorded as used only once the TCP connect succeeded and the hello was written, so
   a re-send after a failed connect (sender's listener not ready, transient loss) retries.
3. Connect TCP to **UDP source IP of the datagram** : `port` (never an address from the
   JSON), timeout 3 s, then send the callback hello and serve §3 as the receiver on that
   socket: the user still decides (invariant: never auto-accept by default), names are
   sanitized (§4), files are published only after the hash matches.

**Callback hello** (receiver → sender, first bytes on the connection):

```
48 4F 50 43 00                     "HOPC" 0x00
00 00 00 58 {"id":…,"token":…}     JSON frame (88 bytes in the vector)
```

JSON: `{"id":"<receiver id>","token":"<token>"}` (Rust/Kotlin/Python write `id` then
`token`; readers must not depend on order). v1 receivers append `"crypto":1` (§7.1). Vectors: `callback_example` in
`docs/test-vectors.json` (datagram, invalid datagrams, exact hello bytes).

**Limits of the scheme.** The token only proves the connector saw the datagram; v0 has no
authentication (ADR 0003) and anyone on the LAN can sniff it. The sender's IP checks
(steps 2–3) stop a LAN host from simply claiming the receiver's `id`, not one that spoofs
the receiver's IP. Conversely, UDP source addresses are not authenticated: a forged
`callback` with a spoofed private/loopback source can make a receiver open a TCP
connection to that IP and an arbitrary `port` and write the fixed `HOPC` hello (≈ 90 bytes,
nothing it read is echoed back) before it waits for `HOPY`. Impact is low (no payload
control, rate-limited as above, local ranges only) and accepted for v0. A send-only node bound to
an ephemeral UDP port does not receive the receiver's multicast announces on that socket.
It MAY, while a callback is pending, also listen to the group on a second socket bound to
47470 with address reuse (the Rust desktop does; on platforms that allow it, bind that
socket to the group address so it never takes unicast meant for a receiver on the same
host); datagrams still leave from the discovery socket. Without it, the sender relies on
its 2 s re-sends: with the 5 s announce period and a ~3 s response window, one of them
falls inside a window within about 10 s. Hearing the announces also needs the sender's
own firewall to let inbound multicast in. See ADR 0006.

## 7. Protocol v1: encryption and device verification

Design and rationale: ADR 0010. Vectors: `v1` in `docs/test-vectors.json` (generated by
`python3 tools/hoppy_ref.py vectors --v1`: keys, the three handshake messages, `h2`, `h`,
SAS, transport messages, QR payload, discovery JSON, emoji table).

### 7.1 Negotiation and compatibility

- v1 nodes add `"crypto":1` to **every** discovery datagram (`hoppy` stays `0`: v0 nodes
  ignore unknown fields, §2 rule 5) and to the callback hello JSON (§6).
- A session starts with `HOPY 0x01` (encrypted, §7.2) or `HOPY 0x00` (v0, §3). v0 nodes close
  the connection on any other version byte.
- **Sender** (the Noise initiator, also on callback connections):

  | What the sender knows about the receiver | Session |
  | --- | --- |
  | It advertised `crypto ≥ 1` (announce or callback hello), **or** a key is remembered for its id (§7.5) | v1. Never falls back: if it closes without answering, the send fails (possible downgrade). |
  | Discovered without `crypto` (a v0 app) and no key remembered | v0, flagged "not encrypted" in the UI |
  | Nothing (an address typed by the user) | v1; if the peer closes or resets the connection before handshake message 2, reconnect and use v0, flagged |

  A callback connection (§6) cannot be retried: the sender decides from the callback hello
  and the announce before writing the hello.
- **Receiver**: reads the hello byte and serves both. A v0 offer is accepted during the
  transition (the UI says "not encrypted") **unless** a key is remembered for
  `offer.sender.id` (that device speaks v1: plaintext would be a downgrade or an
  impersonation) or plaintext is disabled; then it answers
  `{"accept":false,"reason":"encryption_required"}` without asking.
- Transition switch: `ALLOW_PLAINTEXT_DEFAULT` (Rust `transfer.rs`, `CryptoPolicy`; Python
  `--require-encryption`). A later release turns it off: v1 nodes then refuse v0 peers in both
  directions (ADR 0010). UIs MUST show plaintext sessions as "non cifrato" / "not encrypted".

### 7.2 Session layout

```
sender → receiver   "HOPY" 0x01                                    5 bytes hello
sender → receiver   u16 len + Noise message 1  (-> e)               payload: commitment
receiver → sender   u16 len + Noise message 2  (<- e, ee, s, es)    payload: receiver identity + nonce
sender → receiver   u16 len + Noise message 3  (-> s, se)           payload: sender identity + nonce
-- then, both ways: u16 len + Noise transport message (ChaChaPoly, 16-byte tag) --
   the decrypted bytes are exactly the §3 exchange after its hello:
   Offer, Decision, file bytes + SHA-256 per file, Result
```

- On a callback connection (§6) the same bytes follow the `HOPC` hello; roles do not change.
- Every Noise message has a big-endian `u16` length prefix. Handshake messages are at most
  2 048 bytes (receivers reject longer ones before reading them); identity payloads at most
  1 024 bytes. Transport messages are 16–65 535 bytes (≤ 65 519 bytes of plaintext).
- The transport plaintexts form one byte stream per direction: message boundaries carry no
  meaning (the Rust core fills messages up to 65 519 bytes and flushes after each JSON frame).
  An empty plaintext is allowed and ignored. Nonces are the implicit Noise counters, so any
  tampered, dropped, replayed or reordered message fails authentication: the session ends and
  the receiver deletes the partial file (§4 rules still apply inside).
- Timeouts as in §3 (30 s per read, 120 s for the Decision).

### 7.3 Handshake

`Noise_XX_25519_ChaChaPoly_SHA256` (Noise revision 34), prologue = the 5 hello bytes
`48 4F 50 59 01`, static key = the device key (§7.5), no PSK. Payloads (UTF-8 JSON, written
in this field order without spaces; readers must not depend on the order):

| Message | Payload |
| --- | --- |
| 1 (cleartext) | `{"commit":"<hex SHA-256("hoppy-commit-v1" ‖ n_i)>"}` — `n_i` = 32 random bytes, the sender's SAS nonce |
| 2 (encrypted) | `{"id","name","platform","nonce":"<hex n_r>"}` — the receiver; `n_r` = 32 random bytes |
| 3 (encrypted) | `{"id","name","platform","nonce":"<hex n_i>"}` — the sender |

Nonces are 64 lowercase hex characters; ids are 1–128 bytes.

- After message 2 the sender checks that the id equals the discovered id (when it chose a
  discovered device) and the trust state (§7.5): on a changed key it closes the connection
  **before** message 3 (nothing about itself is revealed, no offer is sent).
- After message 3 the receiver checks `SHA-256("hoppy-commit-v1" ‖ n_i)` against the
  commitment (mismatch: close), and later that `Offer.sender.id` equals the handshake id
  (else `invalid_offer`).
- Why the commitment: if the SAS depended only on the handshake transcript, a man in the
  middle could vary its own messages (they come after the honest party's) and try millions of
  variants offline until both screens show the same 24 bits. Here the sender commits to `n_i`
  before the receiver reveals `n_r`, and `h2` is fixed when the receiver speaks, so each
  attack attempt succeeds with probability 2⁻²⁴ (as in ZRTP / Bluetooth numeric comparison).

### 7.4 First encrypted frame

The sender's first JSON frame is either an Offer (§3) or a verification request
`{"verify":"<code>"}` (§7.7). New decline reasons and replies:

| Where | Value | Meaning |
| --- | --- | --- |
| Decision `reason` | `key_changed` | the receiver remembers another key for the sender's id: declined without asking; the receiver warns its user |
| Decision `reason` | `encryption_required` | plaintext offer from a device whose key is remembered, or plaintext disabled |
| Verify reply | `{"ok":true}` / `{"ok":false,"error":"invalid_code"}` / `{"ok":false,"error":"key_changed"}` | §7.7 |

### 7.5 Device keys, fingerprints, trust on first use

- Each device has a static X25519 key pair (RFC 7748; private key = 32 bytes from a CSPRNG,
  clamped when used), created on first run and never sent anywhere. Desktop:
  `<config dir>/hoppy/identity.key` = `{"version":1,"x25519_private":"<64 hex>"}`, owner-only
  (0600); iOS: the `key_file` given to `hoppy-ffi`; Android: app-private storage. The device
  `id` (UUID) does not change; it is bound to the key by trust on first use.
- **Fingerprint**: RFC 4648 base32 (`A–Z2–7`, upper case, no padding) of SHA-256(static
  public key) = 52 characters; shown in groups of 4.
- **Trust store** (one per device, persisted by the platform; desktop `trust.json`):
  `{"version":1,"peers":{"<id>":{"fingerprint","name","first_seen","verified"}}}`
  (`first_seen` in Unix seconds).

| State | Condition | UI |
| --- | --- | --- |
| New | no entry for the id | show the SAS; "Verifica ora" / "Più tardi" |
| Known | entry with this fingerprint, not verified | show the SAS; "Verifica" |
| Verified | entry with this fingerprint, verified | ✓ (no code needed) |
| KeyChanged | entry with **another** fingerprint | block and warn: sender refuses to send, receiver declines `key_changed`; offer "Dimentica dispositivo" |

- A key is remembered (pinned) when a transfer is accepted (receiver: its user accepted;
  sender: the Decision said yes) or when it is verified. A remembered key is never replaced
  automatically: only "forget" (the next contact is New) or an explicit re-trust.

### 7.6 Short authentication string (SAS)

`value` = the first 3 bytes (big-endian) of
`SHA-256("hoppy-sas-v1" ‖ h2 ‖ n_i ‖ n_r)`, where `h2` is the Noise handshake hash right
after message 2 (sender: after reading it; receiver: after writing it). Both forms show the
same 24-bit value:

- **Emoji** (default): 4 emoji of 6 bits each, most significant first:
  `index_k = (value >> (18 − 6k)) & 63` for k = 0…3, from the table below.
- **Numbers** (button "123"): `value` as 8 decimal digits, zero-padded (`0000 0000` …
  `1677 7215`; 2²⁴ < 10⁸, so the mapping is 1:1), shown as two groups of 4.

Two desktops: both users compare by eye and press "Coincide" / "They match": each marks the
other verified. If at least one device is mobile, use the QR code (§7.7) and fall back to the
code when there is no camera or permission. Screen readers read the emoji names (Italian or
English, below).

The table (single source of truth, also `v1.sas_emoji` in `docs/test-vectors.json`; every
implementation copies it and tests against the vectors). Selection rules: exactly 64 single
code points with default emoji presentation (no variation selector, no ZWJ sequence, no skin
tone, no flag), all in Unicode ≤ 9.0 (they render on Windows 10, Android 10, iOS 15); no
arrows or symbols; animals, food, objects and nature with distinct silhouettes; no two items
of the same shape in different colours (one heart/ball/car at most, no coloured circles or
squares); no similar faces; unique names in both languages.

| Index | Emoji | Code point | Italiano | English |
| --- | --- | --- | --- | --- |
| 0 | 🐘 | U+1F418 | elefante | elephant |
| 1 | 🐢 | U+1F422 | tartaruga | turtle |
| 2 | 🐙 | U+1F419 | polpo | octopus |
| 3 | 🦋 | U+1F98B | farfalla | butterfly |
| 4 | 🐌 | U+1F40C | lumaca | snail |
| 5 | 🐝 | U+1F41D | ape | bee |
| 6 | 🐧 | U+1F427 | pinguino | penguin |
| 7 | 🐳 | U+1F433 | balena | whale |
| 8 | 🦀 | U+1F980 | granchio | crab |
| 9 | 🐍 | U+1F40D | serpente | snake |
| 10 | 🦉 | U+1F989 | gufo | owl |
| 11 | 🐪 | U+1F42A | cammello | camel |
| 12 | 🐇 | U+1F407 | coniglio | rabbit |
| 13 | 🐓 | U+1F413 | gallo | rooster |
| 14 | 🐞 | U+1F41E | coccinella | ladybug |
| 15 | 🐖 | U+1F416 | maiale | pig |
| 16 | 🐒 | U+1F412 | scimmia | monkey |
| 17 | 🍎 | U+1F34E | mela | apple |
| 18 | 🍌 | U+1F34C | banana | banana |
| 19 | 🍇 | U+1F347 | uva | grapes |
| 20 | 🥜 | U+1F95C | arachidi | peanuts |
| 21 | 🍋 | U+1F34B | limone | lemon |
| 22 | 🍍 | U+1F34D | ananas | pineapple |
| 23 | 🍿 | U+1F37F | popcorn | popcorn |
| 24 | 🥕 | U+1F955 | carota | carrot |
| 25 | ☕ | U+2615 | caffè | coffee |
| 26 | 🍄 | U+1F344 | fungo | mushroom |
| 27 | 🥑 | U+1F951 | avocado | avocado |
| 28 | 🍕 | U+1F355 | pizza | pizza |
| 29 | 🍔 | U+1F354 | hamburger | hamburger |
| 30 | 🍩 | U+1F369 | ciambella | doughnut |
| 31 | 🍦 | U+1F366 | gelato | ice cream |
| 32 | 🎂 | U+1F382 | torta | cake |
| 33 | 🎲 | U+1F3B2 | dado | dice |
| 34 | 🎁 | U+1F381 | regalo | gift |
| 35 | 🔑 | U+1F511 | chiave | key |
| 36 | 🔔 | U+1F514 | campana | bell |
| 37 | 🎸 | U+1F3B8 | chitarra | guitar |
| 38 | 🎺 | U+1F3BA | tromba | trumpet |
| 39 | ⚽ | U+26BD | pallone | ball |
| 40 | 🚲 | U+1F6B2 | bicicletta | bicycle |
| 41 | 🚀 | U+1F680 | razzo | rocket |
| 42 | ⛵ | U+26F5 | barca a vela | sailboat |
| 43 | ⌛ | U+231B | clessidra | hourglass |
| 44 | 💡 | U+1F4A1 | lampadina | light bulb |
| 45 | 📷 | U+1F4F7 | fotocamera | camera |
| 46 | 🔨 | U+1F528 | martello | hammer |
| 47 | ⚓ | U+2693 | àncora | anchor |
| 48 | 👑 | U+1F451 | corona | crown |
| 49 | 🏠 | U+1F3E0 | casa | house |
| 50 | 🌵 | U+1F335 | cactus | cactus |
| 51 | 🌻 | U+1F33B | girasole | sunflower |
| 52 | 🍀 | U+1F340 | quadrifoglio | four-leaf clover |
| 53 | 🌳 | U+1F333 | albero | tree |
| 54 | 🌴 | U+1F334 | palma | palm tree |
| 55 | 🍁 | U+1F341 | foglia d'acero | maple leaf |
| 56 | ⛄ | U+26C4 | pupazzo di neve | snowman |
| 57 | ⭐ | U+2B50 | stella | star |
| 58 | ⚡ | U+26A1 | fulmine | lightning |
| 59 | 🌈 | U+1F308 | arcobaleno | rainbow |
| 60 | 🌊 | U+1F30A | onda | wave |
| 61 | 🔥 | U+1F525 | fuoco | fire |
| 62 | 🌍 | U+1F30D | mappamondo | globe |
| 63 | 🌋 | U+1F30B | vulcano | volcano |

### 7.7 QR verification

Payload: `hoppy:verify:1:<device id>:<fingerprint>:<code>`, where `code` is 128 random bits
as 32 lowercase hex characters, valid once and only while the QR code is shown (at most
10 minutes). Parsers: exact prefix; split the rest **from the right** (the id may contain
`:`); fingerprint case-insensitive (normalised to upper case); id 1–128 bytes without
control characters.

- Who shows it: the device without a camera (desktop) shows, the phone scans it inside Hoppy;
  phone ↔ phone either way. No camera or no permission: compare the code (§7.6).
- **Scanner**: (1) the fingerprint must equal the key it knows for that id — the key of the
  session on screen (e.g. the accept dialog of a New device) or else the remembered one;
  otherwise it shows "does not match" and sends nothing. (2) It marks that key verified.
  (3) It opens a v1 session to the shower (direct or callback) whose handshake must present
  exactly the scanned fingerprint (otherwise it closes before message 3: the code never
  reaches anyone else) and sends `{"verify":"<code>"}` as the first frame.
- **Shower**: if the code is live (constant-time compare; consumed on use) and the scanner is
  not KeyChanged, it marks the session's peer verified, answers `{"ok":true}`, shows ✓ and
  closes the QR code. The code proves the scanner saw the screen: without it any device on the
  LAN could ask to be marked verified.

### 7.8 Limits of v1

- First contact is trust on first use: an active attacker present at the very first contact is
  only detected if the users compare the code or scan the QR code. 24-bit codes give one
  chance in 16.7 million per attempt.
- Discovery datagrams are still unauthenticated: they can be spoofed to point a sender at the
  wrong address, but the handshake then shows the impostor as New (or blocks it as
  KeyChanged for a remembered id). During the transition, stripping `crypto` from the
  announces of a device never contacted before makes the sender use plaintext — always shown
  as "not encrypted".
- Device names and ids are still broadcast in clear by discovery; file names, sizes and
  contents are encrypted.

## 8. Planned (not in v1)

Resume (per-chunk hashes), folders (relative paths), BLE discovery, Wi-Fi Direct/Aware,
remote relay. See `docs/roadmap.md`.
