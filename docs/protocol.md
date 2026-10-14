# Hoppy wire protocol — v0 (PoC)

Status: **PoC**. Implemented in three places that MUST stay in lockstep:

| Implementation | Path |
| --- | --- |
| Rust core (reference for desktop) | `crates/hoppy-core/src/protocol.rs`, `discovery.rs`, `transfer.rs` |
| Android (Kotlin) | `apps/android/core/src/main/kotlin/app/hoppy/core/` |
| Python reference / test tool | `tools/hoppy_ref.py` |

Any change follows the `hoppy-protocol-change` skill. v0 has **no encryption and no
authentication**: it is only for trusted LANs during the PoC (see ADR 0003).

## 1. Constants

| Name | Value |
| --- | --- |
| Discovery UDP port | `47470` |
| Default transfer TCP port | `47471` (if busy, use any port and announce it) |
| Multicast group (IPv4) | `239.255.72.80` |
| Magic | ASCII `HOPY` = `48 4F 50 59` |
| Protocol version byte | `0x00` |
| Max JSON frame | 1 MiB (1 048 576 bytes) |
| Max files per offer | 10 000 |
| Announce interval | 5 s |
| Peer expiry | 15 s without any message |

## 2. Discovery (UDP)

Every message is one UTF-8 JSON object in one datagram (≤ 1024 bytes).

```json
{"hoppy":0,"type":"announce","id":"5f0c…","name":"Marco-PC","platform":"windows","port":47471}
```

| Field | Type | Notes |
| --- | --- | --- |
| `hoppy` | int | protocol version, `0` |
| `type` | string | `announce`, `query` or `bye` |
| `id` | string | stable device id (UUID v4), persisted on the device |
| `name` | string | human readable, ≤ 64 chars |
| `platform` | string | `windows`, `android`, `linux`, `macos`, `ios` |
| `port` | int | TCP transfer port of the sender |

Rules:

1. The peer address is the **source IP** of the datagram plus `port`.
2. On start a node sends `query` then `announce`; afterwards `announce` every 5 s.
   Targets: multicast `239.255.72.80:47470` on every non-loopback IPv4 interface,
   plus `255.255.255.255:47470` (fallback for networks that drop multicast),
   plus any explicit seed address configured by the user.
3. On `query` the receiver records the sender as a peer **and** replies with an
   `announce` sent by unicast to the datagram's source address and port.
4. On `bye` the peer is removed. Messages carrying the node's own `id` are ignored.
5. Unknown fields MUST be ignored; messages with an unknown `type` or a different
   `hoppy` version MUST be ignored (forward compatibility).
6. `port: 0` marks a **send-only** node (e.g. the desktop "Send to" process while a
   separate receiver runs). It sends `query` every 5 s instead of `announce`,
   binds an ephemeral UDP port so unicast replies reach it, is never listed as a
   peer, and does not answer queries.

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

## 6. Planned for v1 (not in v0)

TLS 1.3 / Noise encryption with pinned device keys and pairing codes, resume
(per-chunk hashes), folders (relative paths), BLE discovery, Wi-Fi Direct/Aware,
remote relay. See `docs/roadmap.md`.
