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
| Callback magic (§6) | ASCII `HOPC` = `48 4F 50 43` |
| Protocol version byte | `0x00` |
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
   skipping its own address and the network/broadcast addresses, only on broadcast-capable
   interfaces (no cellular, no point-to-point VPN). At most 1024 targets, paced (≈ 2 ms
   apart). At start, then every 10 s (shorter than the 15 s peer expiry, so nodes that
   only hear it through the sweep keep listing it), and on user refresh (at most every 3 s).
   Seeds (rule 2) still get every message.
2. Receivers answer each `query` with a unicast `announce` and record the sweeping node as a
   peer when its `port != 0` (rule 3): both sides learn each other.
3. While receiving (`port != 0`) it also sends `query` to each known peer every 5 s, and a
   node that does not multicast sends `bye`/`announce` for state changes to its known peers
   by unicast too (e.g. `bye` when it stops receiving or quits).
4. A send-only node on an ephemeral port (rule 6) never hears these nodes' unicast
   `announce`, so desktop send-only processes without explicit seeds sweep as well.

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
`token`; readers must not depend on order). Vectors: `callback_example` in
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

## 7. Planned for v1 (not in v0)

TLS 1.3 / Noise encryption with pinned device keys and pairing codes, resume
(per-chunk hashes), folders (relative paths), BLE discovery, Wi-Fi Direct/Aware,
remote relay. See `docs/roadmap.md`.
