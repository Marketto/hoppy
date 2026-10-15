#!/usr/bin/env python3
"""Hoppy protocol reference implementation, v0 + v1 (stdlib only).

Independent second implementation of docs/protocol.md, used to:
  * check conformance of the Rust core and the Android app (interop tests),
  * debug real devices from any machine with Python 3.9+.

v1 (spec §7) is implemented from scratch here: X25519 (RFC 7748), ChaCha20-Poly1305
(RFC 8439), HMAC/HKDF over hashlib, Noise_XX_25519_ChaChaPoly_SHA256, the SAS and the QR
payload. Pure Python is slow (~1 MB/s encrypted): use small files.

Usage:
  hoppy_ref.py selftest                    run docs/test-vectors.json + RFC vectors
  hoppy_ref.py peers [--wait 3]            list nearby devices
  hoppy_ref.py receive DIR [--port P]      accept everything into DIR (trusted LAN only);
                                           also answers callback requests (spec §6).
                                           --show-qr prints a QR payload (spec §7.7)
  hoppy_ref.py send HOST[:PORT] FILE...    send files; falls back to a callback if the
                                           TCP connect fails (--no-direct: callback only)
  hoppy_ref.py abort-handshakes HOST:PORT N   play a code-guessing attacker (spec §7.3): N v1
                                           handshakes abandoned after message 2; prints how
                                           many were answered and how many refused ("busy")
  hoppy_ref.py vectors [--v1]              print the byte-level example of spec §5, or
                                           the deterministic v1 vectors as JSON
Common options: --v0 behaves exactly like a v0 (old) node: no encryption, no `crypto`
field; --key FILE / --trust FILE keep the device key / trust store (default: in memory);
--require-encryption refuses plaintext.
"""
from __future__ import annotations

import argparse
import base64
import hashlib
import hmac
import ipaddress
import json
import os
import select
import socket
import struct
import sys
import threading
import time
import unicodedata
import uuid
from pathlib import Path

MAGIC = b"HOPY"
CALLBACK_MAGIC = b"HOPC"
VERSION = 0
VERSION_V1 = 1  # hello byte of an encrypted session (spec §7.2)
CRYPTO_VERSION = 1  # `crypto` field of v1 datagrams / callback hellos
DISCOVERY_PORT = 47470
TRANSFER_PORT = 47471
MCAST = "239.255.72.80"
MAX_FRAME = 1 << 20
MAX_FILES = 10_000
CHUNK = 256 * 1024
# Callback (spec §6); same values as Rust/Kotlin.
DIRECT_CONNECT_TIMEOUT = 3.0
CALLBACK_WINDOW = 20.0
CALLBACK_RESEND = 2.0
CALLBACK_CONNECT_TIMEOUT = 3.0
CALLBACK_HELLO_TIMEOUT = 3.0  # one deadline for the whole hello, not per read
MAX_CALLBACK_HELLO_FRAME = 1024
MAX_CALLBACKS_PER_MINUTE = 10  # per source IP
MAX_CALLBACKS_PER_MINUTE_TOTAL = 30
TOKEN_MEMORY = 120.0
ROOT = Path(__file__).resolve().parent.parent
RESERVED = {"CON", "PRN", "AUX", "NUL", *(f"COM{i}" for i in range(1, 10)), *(f"LPT{i}" for i in range(1, 10))}


# ---------------------------------------------------------------- framing
def read_exact(sock: socket.socket, n: int, deadline: float | None = None) -> bytes:
    """Reads n bytes; with a `deadline` (time.monotonic()) the whole read must finish by then."""
    buf = bytearray()
    while len(buf) < n:
        if deadline is not None:
            left = deadline - time.monotonic()
            if left <= 0:
                raise TimeoutError("read deadline passed")
            sock.settimeout(left)
        chunk = sock.recv(n - len(buf))
        if not chunk:
            raise ConnectionError("connection closed")
        buf += chunk
    return bytes(buf)


def write_json(sock: socket.socket, obj: dict) -> None:
    body = json.dumps(obj, separators=(",", ":"), ensure_ascii=False).encode()
    if len(body) > MAX_FRAME:
        raise ValueError("frame too large")
    sock.sendall(struct.pack(">I", len(body)) + body)


def read_json(sock: socket.socket, max_len: int = MAX_FRAME, deadline: float | None = None) -> dict:
    (n,) = struct.unpack(">I", read_exact(sock, 4, deadline))
    if n > max_len:
        raise ValueError(f"frame of {n} bytes exceeds limit")
    obj = json.loads(read_exact(sock, n, deadline))
    if not isinstance(obj, dict):
        raise ValueError("frame is not a JSON object")
    return obj


# ---------------------------------------------------------------- v1 crypto primitives
# Pure Python, constant time NOT guaranteed: a reference/test tool, not for production.
P25519 = 2**255 - 19


def x25519(k: bytes, u: bytes) -> bytes:
    """RFC 7748 §5 X25519 (Montgomery ladder)."""
    kb = bytearray(k)
    kb[0] &= 248
    kb[31] &= 127
    kb[31] |= 64
    scalar = int.from_bytes(kb, "little")
    x1 = int.from_bytes(u, "little") & ((1 << 255) - 1)
    x2, z2, x3, z3, swap, p = 1, 0, x1, 1, 0, P25519
    for t in range(254, -1, -1):
        bit = (scalar >> t) & 1
        if swap ^ bit:
            x2, x3, z2, z3 = x3, x2, z3, z2
        swap = bit
        a, b = (x2 + z2) % p, (x2 - z2) % p
        aa, bb = a * a % p, b * b % p
        e = (aa - bb) % p
        c, d = (x3 + z3) % p, (x3 - z3) % p
        da, cb = d * a % p, c * b % p
        x3, z3 = (da + cb) ** 2 % p, x1 * (da - cb) ** 2 % p
        x2, z2 = aa * bb % p, e * (aa + 121665 * e) % p
    if swap:
        x2, z2 = x3, z3
    return (x2 * pow(z2, p - 2, p) % p).to_bytes(32, "little")


def x25519_public(private: bytes) -> bytes:
    return x25519(private, (9).to_bytes(32, "little"))


M32 = 0xFFFFFFFF


def _chacha_block(key_words: tuple, counter: int, nonce_words: tuple) -> bytes:
    """RFC 8439 §2.3 block function."""
    init = (0x61707865, 0x3320646E, 0x79622D32, 0x6B206574, *key_words, counter & M32, *nonce_words)
    x = list(init)
    for _ in range(10):
        for a, b, c, d in ((0, 4, 8, 12), (1, 5, 9, 13), (2, 6, 10, 14), (3, 7, 11, 15),
                           (0, 5, 10, 15), (1, 6, 11, 12), (2, 7, 8, 13), (3, 4, 9, 14)):
            xa, xb, xc, xd = x[a], x[b], x[c], x[d]
            xa = (xa + xb) & M32; xd ^= xa; xd = ((xd << 16) | (xd >> 16)) & M32
            xc = (xc + xd) & M32; xb ^= xc; xb = ((xb << 12) | (xb >> 20)) & M32
            xa = (xa + xb) & M32; xd ^= xa; xd = ((xd << 8) | (xd >> 24)) & M32
            xc = (xc + xd) & M32; xb ^= xc; xb = ((xb << 7) | (xb >> 25)) & M32
            x[a], x[b], x[c], x[d] = xa, xb, xc, xd
    return struct.pack("<16I", *((x[i] + init[i]) & M32 for i in range(16)))


def chacha20(key: bytes, counter: int, nonce: bytes, data: bytes) -> bytes:
    """RFC 8439 §2.4 encryption (XOR with the key stream starting at block `counter`)."""
    if not data:
        return b""
    kw, nw = struct.unpack("<8I", key), struct.unpack("<3I", nonce)
    blocks = (len(data) + 63) // 64
    stream = b"".join(_chacha_block(kw, counter + i, nw) for i in range(blocks))[: len(data)]
    return (int.from_bytes(data, "little") ^ int.from_bytes(stream, "little")).to_bytes(len(data), "little")


def poly1305(key: bytes, msg: bytes) -> bytes:
    """RFC 8439 §2.5."""
    r = int.from_bytes(key[:16], "little") & 0x0FFFFFFC0FFFFFFC0FFFFFFC0FFFFFFF
    s = int.from_bytes(key[16:32], "little")
    p, acc = (1 << 130) - 5, 0
    for i in range(0, len(msg), 16):
        acc = (acc + int.from_bytes(msg[i:i + 16] + b"\x01", "little")) * r % p
    return ((acc + s) & ((1 << 128) - 1)).to_bytes(16, "little")


def _aead_mac_data(aad: bytes, ct: bytes) -> bytes:
    pad = lambda b: b"\x00" * (-len(b) % 16)  # noqa: E731
    return aad + pad(aad) + ct + pad(ct) + struct.pack("<QQ", len(aad), len(ct))


def aead_encrypt(key: bytes, nonce: bytes, aad: bytes, plaintext: bytes) -> bytes:
    """RFC 8439 §2.8 AEAD_CHACHA20_POLY1305: ciphertext ‖ 16-byte tag."""
    otk = chacha20(key, 0, nonce, b"\x00" * 32)
    ct = chacha20(key, 1, nonce, plaintext)
    return ct + poly1305(otk, _aead_mac_data(aad, ct))


def aead_decrypt(key: bytes, nonce: bytes, aad: bytes, sealed: bytes) -> bytes:
    if len(sealed) < 16:
        raise ValueError("ciphertext too short")
    ct, tag = sealed[:-16], sealed[-16:]
    otk = chacha20(key, 0, nonce, b"\x00" * 32)
    if not hmac.compare_digest(poly1305(otk, _aead_mac_data(aad, ct)), tag):
        raise ValueError("authentication failed (tampered or corrupted data)")
    return chacha20(key, 1, nonce, ct)


def hmac_sha256(key: bytes, data: bytes) -> bytes:
    return hmac.new(key, data, hashlib.sha256).digest()


def hkdf_rfc5869(salt: bytes, ikm: bytes, info: bytes, length: int) -> bytes:
    prk = hmac_sha256(salt or b"\x00" * 32, ikm)
    out, block, i = b"", b"", 1
    while len(out) < length:
        block = hmac_sha256(prk, block + info + bytes([i]))
        out += block
        i += 1
    return out[:length]


# ---------------------------------------------------------------- Noise XX (spec §7.3)
NOISE_NAME = b"Noise_XX_25519_ChaChaPoly_SHA256"
MAX_NOISE_MESSAGE = 65535
MAX_PLAINTEXT = MAX_NOISE_MESSAGE - 16
MAX_HANDSHAKE_MESSAGE = 2048
PROLOGUE = MAGIC + bytes([VERSION_V1])
SESSION_DEADLINE = 130.0  # sender: handshake + Decision, one wall-clock deadline (§7.3)
RECEIVER_HANDSHAKE_DEADLINE = 10.0  # receiver: messages 1, 3 and the first frame
MAX_ABORTS_PER_IP = 5  # aborted handshakes per source IP in any 60 s (§7.3)
MAX_ABORTS_TOTAL = 20  # ... from all sources together
MAX_NAME_CHARS, MAX_PLATFORM_CHARS, MAX_ID_BYTES = 64, 16, 128


def is_low_order(public: bytes) -> bool:
    """Spec §7.3: a small-order X25519 point (any clamped scalar maps it to zero)."""
    return len(public) != 32 or x25519(b"\x5a" * 32, public) == b"\x00" * 32


def _noise_nonce(n: int) -> bytes:
    return b"\x00" * 4 + struct.pack("<Q", n)


class CipherState:
    def __init__(self, key: bytes | None = None) -> None:
        self.k, self.n = key, 0

    def encrypt(self, ad: bytes, pt: bytes) -> bytes:
        if self.k is None:
            return pt
        ct = aead_encrypt(self.k, _noise_nonce(self.n), ad, pt)
        self.n += 1
        return ct

    def decrypt(self, ad: bytes, ct: bytes) -> bytes:
        if self.k is None:
            return ct
        pt = aead_decrypt(self.k, _noise_nonce(self.n), ad, ct)
        self.n += 1
        return pt


def _hkdf2(ck: bytes, ikm: bytes) -> tuple[bytes, bytes]:
    temp = hmac_sha256(ck, ikm)
    o1 = hmac_sha256(temp, b"\x01")
    return o1, hmac_sha256(temp, o1 + b"\x02")


class NoiseXX:
    """One side of Noise_XX_25519_ChaChaPoly_SHA256 (Noise specification rev. 34, sections 5 and 7.4)."""

    def __init__(self, initiator: bool, static_private: bytes, ephemeral_private: bytes | None = None) -> None:
        self.initiator = initiator
        self.s = static_private
        self.s_pub = x25519_public(static_private)
        self.e = ephemeral_private or os.urandom(32)
        self.e_pub = x25519_public(self.e)
        self.h = NOISE_NAME  # exactly HASHLEN bytes: used as is
        self.ck = self.h
        self.cs = CipherState()
        self.re = self.rs = None
        self.mix_hash(PROLOGUE)

    def mix_hash(self, data: bytes) -> None:
        self.h = hashlib.sha256(self.h + data).digest()

    def mix_key(self, ikm: bytes) -> None:
        self.ck, temp = _hkdf2(self.ck, ikm)
        self.cs = CipherState(temp)

    def enc_hash(self, pt: bytes) -> bytes:
        ct = self.cs.encrypt(self.h, pt)
        self.mix_hash(ct)
        return ct

    def dec_hash(self, ct: bytes) -> bytes:
        pt = self.cs.decrypt(self.h, ct)
        self.mix_hash(ct)
        return pt

    def _key_len(self) -> int:
        return 48 if self.cs.k is not None else 32

    # -> e
    def write_msg1(self, payload: bytes) -> bytes:
        self.mix_hash(self.e_pub)
        return self.e_pub + self.enc_hash(payload)

    def read_msg1(self, msg: bytes) -> bytes:
        if len(msg) < 32 or is_low_order(msg[:32]):
            raise ValueError("bad handshake message 1")
        self.re = msg[:32]
        self.mix_hash(self.re)
        return self.dec_hash(msg[32:])

    # <- e, ee, s, es
    def write_msg2(self, payload: bytes) -> bytes:
        self.mix_hash(self.e_pub)
        self.mix_key(x25519(self.e, self.re))
        enc_s = self.enc_hash(self.s_pub)
        self.mix_key(x25519(self.s, self.re))
        return self.e_pub + enc_s + self.enc_hash(payload)

    def read_msg2(self, msg: bytes) -> bytes:
        if len(msg) < 32 + 48 + 16 or is_low_order(msg[:32]):
            raise ValueError("bad handshake message 2")
        self.re = msg[:32]
        self.mix_hash(self.re)
        self.mix_key(x25519(self.e, self.re))
        self.rs = self.dec_hash(msg[32:80])
        if is_low_order(self.rs):
            raise ValueError("low-order static key")
        self.mix_key(x25519(self.e, self.rs))
        return self.dec_hash(msg[80:])

    # -> s, se
    def write_msg3(self, payload: bytes) -> bytes:
        enc_s = self.enc_hash(self.s_pub)
        self.mix_key(x25519(self.s, self.re))
        return enc_s + self.enc_hash(payload)

    def read_msg3(self, msg: bytes) -> bytes:
        if len(msg) < 48 + 16:
            raise ValueError("bad handshake message 3")
        self.rs = self.dec_hash(msg[:48])
        if is_low_order(self.rs):
            raise ValueError("low-order static key")
        self.mix_key(x25519(self.e, self.rs))
        return self.dec_hash(msg[48:])

    def split(self) -> tuple[CipherState, CipherState]:
        """(send, receive) cipher states for this side."""
        k1, k2 = _hkdf2(self.ck, b"")
        c1, c2 = CipherState(k1), CipherState(k2)
        return (c1, c2) if self.initiator else (c2, c1)


# ---------------------------------------------------------------- v1 identity, SAS, QR
BASE32 = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567"
QR_PREFIX = "hoppy:verify:1:"
QR_CODE_TTL = 600.0
# Spec §7.6 table (same as docs/test-vectors.json "v1.sas_emoji"; selftest compares).
SAS_EMOJI = [
    ("🐘", "elefante", "elephant"), ("🐢", "tartaruga", "turtle"), ("🐙", "polpo", "octopus"),
    ("🦋", "farfalla", "butterfly"), ("🐌", "lumaca", "snail"), ("🐝", "ape", "bee"),
    ("🐧", "pinguino", "penguin"), ("🐳", "balena", "whale"), ("🦀", "granchio", "crab"),
    ("🐍", "serpente", "snake"), ("🦉", "gufo", "owl"), ("🐪", "cammello", "camel"),
    ("🐇", "coniglio", "rabbit"), ("🐓", "gallo", "rooster"), ("🐞", "coccinella", "ladybug"),
    ("🐖", "maiale", "pig"), ("🐒", "scimmia", "monkey"), ("🍎", "mela", "apple"),
    ("🍌", "banana", "banana"), ("🍇", "uva", "grapes"), ("🥜", "arachidi", "peanuts"),
    ("🍋", "limone", "lemon"), ("🍍", "ananas", "pineapple"), ("🍿", "popcorn", "popcorn"),
    ("🥕", "carota", "carrot"), ("☕", "caffè", "coffee"), ("🍄", "fungo", "mushroom"),
    ("🥑", "avocado", "avocado"), ("🍕", "pizza", "pizza"), ("🍔", "hamburger", "hamburger"),
    ("🍩", "ciambella", "doughnut"), ("🍦", "gelato", "ice cream"), ("🎂", "torta", "cake"),
    ("🎲", "dado", "dice"), ("🎁", "regalo", "gift"), ("🔑", "chiave", "key"),
    ("🔔", "campana", "bell"), ("🎸", "chitarra", "guitar"), ("🎺", "tromba", "trumpet"),
    ("⚽", "pallone", "ball"), ("🚲", "bicicletta", "bicycle"), ("🚀", "razzo", "rocket"),
    ("⛵", "barca a vela", "sailboat"), ("⌛", "clessidra", "hourglass"), ("💡", "lampadina", "light bulb"),
    ("📷", "fotocamera", "camera"), ("🔨", "martello", "hammer"), ("⚓", "àncora", "anchor"),
    ("👑", "corona", "crown"), ("🏠", "casa", "house"), ("🌵", "cactus", "cactus"),
    ("🌻", "girasole", "sunflower"), ("🍀", "quadrifoglio", "four-leaf clover"), ("🌳", "albero", "tree"),
    ("🌴", "palma", "palm tree"), ("🍁", "foglia d'acero", "maple leaf"), ("⛄", "pupazzo di neve", "snowman"),
    ("⭐", "stella", "star"), ("⚡", "fulmine", "lightning"), ("🌈", "arcobaleno", "rainbow"),
    ("🌊", "onda", "wave"), ("🔥", "fuoco", "fire"), ("🌍", "mappamondo", "globe"),
    ("🌋", "vulcano", "volcano"),
]


def base32(data: bytes) -> str:
    """RFC 4648 base32, upper case, no padding."""
    return base64.b32encode(data).decode().rstrip("=")


def fingerprint(public: bytes) -> str:
    """Spec §7.5: base32(SHA-256(static public key)), 52 characters."""
    return base32(hashlib.sha256(public).digest())


def commit(nonce: bytes) -> bytes:
    return hashlib.sha256(b"hoppy-commit-v1" + nonce).digest()


def sas_value(h2: bytes, initiator_nonce: bytes, responder_nonce: bytes) -> int:
    """Spec §7.6: first 24 bits of SHA-256("hoppy-sas-v1" ‖ h2 ‖ n_i ‖ n_r)."""
    return int.from_bytes(hashlib.sha256(b"hoppy-sas-v1" + h2 + initiator_nonce + responder_nonce).digest()[:3], "big")


def sas_render(value: int) -> dict:
    value &= 0xFFFFFF
    idx = [(value >> shift) & 63 for shift in (18, 12, 6, 0)]
    d = f"{value:08d}"
    return {"value": value, "emoji": idx, "text": "".join(SAS_EMOJI[i][0] for i in idx),
            "digits": f"{d[:4]} {d[4:]}", "it": [SAS_EMOJI[i][1] for i in idx], "en": [SAS_EMOJI[i][2] for i in idx]}


def qr_payload(device_id: str, fp: str, code: str) -> str:
    return f"{QR_PREFIX}{device_id}:{fp}:{code}"


def parse_qr(text: str) -> dict | None:
    text = text.strip()
    if not text.startswith(QR_PREFIX):
        return None
    parts = text[len(QR_PREFIX):].rsplit(":", 2)
    if len(parts) != 3:
        return None
    device_id, fp, code = parts[0], parts[1].upper(), parts[2]
    ok = (device_id and len(device_id.encode()) <= 128 and not any(ord(c) < 32 or ord(c) == 127 for c in device_id)
          and len(fp) == 52 and all(c in BASE32 for c in fp)
          and len(code) == 32 and all(c in "0123456789abcdef" for c in code))
    return {"id": device_id, "fingerprint": fp, "code": code} if ok else None


def write_private(path: Path, data: bytes, exclusive: bool = False) -> None:
    """Atomic write: a unique temp file created 0600 before any byte is written, then rename
    (or, with `exclusive`, a link that fails if `path` appeared meanwhile: then keep that)."""
    tmp = path.with_name(f".{path.name}.{os.getpid()}.{os.urandom(8).hex()}.tmp")
    fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    try:
        with os.fdopen(fd, "wb") as f:
            f.write(data)
            f.flush()
            os.fsync(f.fileno())
        if exclusive:
            try:
                os.link(tmp, path)
            except FileExistsError:
                pass
        else:
            os.replace(tmp, path)
    finally:
        if tmp.exists():
            tmp.unlink()


def hidden_char(c: str) -> bool:
    """Characters stripped from network text before showing it (Rust `display_text`)."""
    cat = unicodedata.category(c)
    return cat in ("Cc", "Zl", "Zp") or (cat == "Cf" and c not in "\u200c\u200d")


def display_text(raw: str, limit: int) -> str:
    clean = "".join(c for c in raw if not hidden_char(c)).strip()
    if len(clean) <= limit:
        return clean
    return clean[: limit - 1].rstrip() + "…"


class HandshakeGuard:
    """Spec §7.3: v1 sessions that end after message 2 without a first frame are aborted
    handshakes; at most 5 per source IP and 20 in total per rolling minute, with a per-IP
    backoff (60 s, doubling, ≤ 1 h). Same rules as Rust `guard.rs`."""

    def __init__(self) -> None:
        self.lock = threading.Lock()
        self.aborts: dict[str, list[float]] = {}
        self.total: list[float] = []
        self.strikes: dict[str, int] = {}
        self.blocked_until: dict[str, float] = {}

    def _prune(self, now: float) -> None:
        self.total = [t for t in self.total if now - t < 60]
        self.aborts = {ip: kept for ip, ts in self.aborts.items() if (kept := [t for t in ts if now - t < 60])}

    def admit(self, ip: str, now: float | None = None) -> bool:
        now = time.monotonic() if now is None else now
        with self.lock:
            self._prune(now)
            return (len(self.total) < MAX_ABORTS_TOTAL and self.blocked_until.get(ip, 0) <= now
                    and len(self.aborts.get(ip, [])) < MAX_ABORTS_PER_IP)

    def record_abort(self, ip: str, now: float | None = None) -> int:
        now = time.monotonic() if now is None else now
        with self.lock:
            self._prune(now)
            self.total.append(now)
            ts = self.aborts.setdefault(ip, [])
            ts.append(now)
            if len(ts) >= MAX_ABORTS_PER_IP:
                self.strikes[ip] = self.strikes.get(ip, 0) + 1
                self.blocked_until[ip] = now + min(60 * 2 ** min(self.strikes[ip] - 1, 6), 3600)
            return len(ts)


class Identity:
    """Device key (static X25519) + trust store + live QR codes; optionally persisted."""

    def __init__(self, device: dict, key_file: Path | None = None, trust_file: Path | None = None,
                 v0: bool = False, require_encryption: bool = False) -> None:
        self.device, self.v0, self.require_encryption = device, v0, require_encryption
        self.lock = threading.Lock()
        self.private = None
        if key_file and key_file.exists():
            data = json.loads(key_file.read_text())
            if data.get("version") != 1:
                raise ValueError(f"key file version {data.get('version')}: not overwriting it")
            self.private = bytes.fromhex(data["x25519_private"])
        if self.private is None:
            self.private = os.urandom(32)
            if key_file:
                write_private(key_file, json.dumps({"version": 1, "x25519_private": self.private.hex()}).encode(),
                              exclusive=True)
                self.private = bytes.fromhex(json.loads(key_file.read_text())["x25519_private"])
        self.public = x25519_public(self.private)
        self.fingerprint = fingerprint(self.public)
        self.trust_file = trust_file
        self.peers: dict = {}
        if trust_file and trust_file.exists():
            try:
                self.peers = json.loads(trust_file.read_text()).get("peers", {})
            except (ValueError, AttributeError):
                trust_file.rename(trust_file.with_suffix(".json.bad"))  # damaged: set aside
                print(f"warning: {trust_file} was damaged; set aside", file=sys.stderr)
        self.qr_codes: dict[str, float] = {}
        self.guard = HandshakeGuard()

    def _save(self) -> None:
        if self.trust_file:
            write_private(self.trust_file, json.dumps({"version": 1, "peers": self.peers}, indent=2,
                                                       ensure_ascii=False).encode())

    def state(self, peer_id: str, fp: str) -> str:
        with self.lock:
            e = self.peers.get(peer_id)
        if e is None:
            return "new"
        if e["fingerprint"] != fp:
            return "key_changed"
        return "verified" if e["verified"] else "known"

    def pin(self, peer: dict, verified: bool = False, addr: str | None = None) -> None:
        with self.lock:
            e = self.peers.get(peer["id"])
            if e is None:
                e = self.peers[peer["id"]] = {"fingerprint": peer["fingerprint"], "name": peer["name"],
                                              "first_seen": int(time.time()), "verified": verified}
            elif e["fingerprint"] == peer["fingerprint"]:
                e["name"], e["verified"] = peer["name"], e["verified"] or verified
            else:
                return
            if addr:
                e["addr"] = addr
            self._save()

    def encrypted_at(self, addr: str) -> bool:
        """A remembered device completed an encrypted transfer at this `ip:port` (§7.1)."""
        with self.lock:
            return any(e.get("addr") == addr for e in self.peers.values())

    def name_taken_by_other(self, peer_id: str, name: str) -> bool:
        n = name.strip().lower()
        with self.lock:
            return bool(n) and any(i != peer_id and str(e.get("name", "")).strip().lower() == n
                                   for i, e in self.peers.items())

    def begin_qr(self) -> str:
        code = os.urandom(16).hex()
        with self.lock:
            self.qr_codes[code] = time.monotonic()
        return qr_payload(self.device["id"], self.fingerprint, code)

    def take_qr_code(self, code: str) -> bool:
        with self.lock:
            now = time.monotonic()
            self.qr_codes = {c: t for c, t in self.qr_codes.items() if now - t < QR_CODE_TTL}
            for c in list(self.qr_codes):
                if hmac.compare_digest(c.encode(), str(code).encode()):
                    del self.qr_codes[c]
                    return True
        return False


def identity_payload(device: dict, nonce: bytes) -> bytes:
    body = {"id": device["id"], "name": device["name"], "platform": device["platform"], "nonce": nonce.hex()}
    return json.dumps(body, separators=(",", ":"), ensure_ascii=False).encode()


def parse_identity(payload: bytes) -> tuple[dict, bytes]:
    p = json.loads(payload)
    if not isinstance(p, dict):
        raise ValueError("bad handshake payload")
    ident, name, platform = p.get("id"), p.get("name"), p.get("platform")
    if (not isinstance(ident, str) or not ident or len(ident.encode()) > MAX_ID_BYTES
            or display_text(ident, MAX_ID_BYTES) != ident):
        raise ValueError("bad device id in handshake")
    if not isinstance(name, str) or not isinstance(platform, str):
        raise ValueError("bad device name in handshake")
    n = p.get("nonce")
    if not isinstance(n, str) or len(n) != 64 or any(c not in "0123456789abcdef" for c in n):
        raise ValueError("bad handshake nonce")
    nonce = bytes.fromhex(n)
    return {"id": ident, "name": display_text(name, MAX_NAME_CHARS),
            "platform": display_text(platform, MAX_PLATFORM_CHARS)}, nonce


def read_frame(sock, max_len: int, deadline: float | None = None) -> bytes | None:
    """`u16 length` + bytes; None on a clean EOF before the length."""
    if deadline is not None:
        left = deadline - time.monotonic()
        if left <= 0:
            raise TimeoutError("no answer in time")
        sock.settimeout(left)
    first = sock.recv(1)
    if not first:
        return None
    (n,) = struct.unpack(">H", first + read_exact(sock, 1, deadline))
    if n > max_len:
        raise ValueError(f"Noise message of {n} bytes exceeds {max_len}")
    return read_exact(sock, n, deadline)


def frame(msg: bytes) -> bytes:
    return struct.pack(">H", len(msg)) + msg


class NoiseSocket:
    """The v1 byte stream over a socket: same sendall/recv/settimeout as a socket."""

    def __init__(self, sock: socket.socket, send: CipherState, recv: CipherState) -> None:
        self.sock, self.tx, self.rx, self.buf = sock, send, recv, b""
        self.deadline: float | None = None  # optional wall-clock limit for reads

    def sendall(self, data: bytes) -> None:
        for i in range(0, len(data), MAX_PLAINTEXT):
            self.sock.sendall(frame(self.tx.encrypt(b"", data[i:i + MAX_PLAINTEXT])))

    def recv(self, n: int) -> bytes:
        if not self.buf:
            msg = read_frame(self.sock, MAX_NOISE_MESSAGE, self.deadline)
            if msg is None:
                return b""
            if len(msg) <= 16:  # spec §7.2: empty transport messages are never sent
                raise ValueError("empty or short Noise transport message")
            self.buf = self.rx.decrypt(b"", msg)
        out, self.buf = self.buf[:n], self.buf[n:]
        return out

    def settimeout(self, t) -> None:
        self.sock.settimeout(t)

    def close(self) -> None:
        self.sock.close()


class PeerClosed(Exception):
    """The peer closed right after our v1 hello: a v0 node."""


def initiate(sock: socket.socket, ident: Identity, expected_id: str | None = None,
             expect_fp: str | None = None, deadline: float | None = None) -> tuple[NoiseSocket, dict]:
    """Initiator side of spec §7.3. Returns the encrypted stream and the session info."""
    hs = NoiseXX(True, ident.private)
    nonce = os.urandom(32)
    commit_payload = json.dumps({"commit": commit(nonce).hex()}, separators=(",", ":")).encode()
    try:
        sock.sendall(PROLOGUE + frame(hs.write_msg1(commit_payload)))
        msg2 = read_frame(sock, MAX_HANDSHAKE_MESSAGE, deadline)
    except (ConnectionResetError, ConnectionAbortedError, BrokenPipeError, ConnectionError) as e:
        raise PeerClosed() from e
    if msg2 is None:
        raise PeerClosed()
    if not msg2:
        raise ValueError("busy: the receiver is limiting connections, try again in a minute")
    peer, r_nonce = parse_identity(hs.read_msg2(msg2))
    h2 = hs.h
    peer["fingerprint"] = fingerprint(hs.rs)
    if expected_id and peer["id"] != expected_id:
        raise ValueError(f"the device answered as {peer['id']}, not {expected_id}")
    state = ident.state(peer["id"], peer["fingerprint"])
    if expect_fp is not None and expect_fp != peer["fingerprint"]:
        raise ValueError("verification failed: another key than the scanned code")
    if expect_fp is None and state == "key_changed":
        raise ValueError(f"the security key of {peer['name']} has changed: not sending")
    sock.sendall(frame(hs.write_msg3(identity_payload(ident.device, nonce))))
    tx, rx = hs.split()
    info = {"encrypted": True, "peer": peer, "trust": state, "sas": sas_render(sas_value(h2, nonce, r_nonce)),
            "name_matches_known": state == "new" and ident.name_taken_by_other(peer["id"], peer["name"])}
    stream = NoiseSocket(sock, tx, rx)
    stream.deadline = deadline
    return stream, info


class Refused(Exception):
    """The aborted-handshake limit refused this connection (spec §7.3)."""


def respond(sock: socket.socket, ident: Identity) -> tuple[NoiseSocket, dict, dict]:
    """Responder side of spec §7.3, after the `HOPY 0x01` hello was read, up to and including
    the first encrypted frame (returned too). Sessions that end after message 2 without a
    first frame are recorded as aborted handshakes."""
    ip = sock.getpeername()[0]
    deadline = time.monotonic() + RECEIVER_HANDSHAKE_DEADLINE
    admitted = ident.guard.admit(ip)
    hs = NoiseXX(False, ident.private)
    nonce = os.urandom(32)
    msg1 = read_frame(sock, MAX_HANDSHAKE_MESSAGE, deadline)
    if msg1 is None:
        raise ConnectionError("closed during the handshake")
    if not admitted:
        sock.sendall(frame(b""))  # "busy, not now"
        raise Refused(f"too many interrupted handshakes from {ip}: someone may be guessing codes")
    c = json.loads(hs.read_msg1(msg1)).get("commit")
    if not isinstance(c, str) or len(c) != 64:
        raise ValueError("bad commitment")
    their_commit = bytes.fromhex(c)
    sock.sendall(frame(hs.write_msg2(identity_payload(ident.device, nonce))))
    h2 = hs.h
    try:
        msg3 = read_frame(sock, MAX_HANDSHAKE_MESSAGE, deadline)
        if msg3 is None:
            raise ConnectionError("closed during the handshake")
        peer, i_nonce = parse_identity(hs.read_msg3(msg3))
        if not hmac.compare_digest(commit(i_nonce), their_commit):
            raise ValueError("SAS commitment mismatch")
        tx, rx = hs.split()
        stream = NoiseSocket(sock, tx, rx)
        stream.deadline = deadline
        first = read_json(stream)
        stream.deadline = None
    except (OSError, ValueError, KeyError):
        if ident.guard.record_abort(ip) >= MAX_ABORTS_PER_IP:
            print(f"warning: repeated interrupted handshakes from {ip}", file=sys.stderr)
        raise
    peer["fingerprint"] = fingerprint(hs.rs)
    state = ident.state(peer["id"], peer["fingerprint"])
    info = {"encrypted": True, "peer": peer, "trust": state, "sas": sas_render(sas_value(h2, i_nonce, nonce)),
            "name_matches_known": state == "new" and ident.name_taken_by_other(peer["id"], peer["name"])}
    return stream, info, first


def describe(info: dict) -> str:
    if not info.get("encrypted"):
        return "NOT encrypted connection"
    sas = info["sas"]
    return f"encrypted · {info['trust']} · {sas['text']} ({sas['digits']}) · {info['peer']['fingerprint'][:8]}…"


# ---------------------------------------------------------------- names (spec §4)
def sanitize(raw: str) -> str | None:
    last = raw.replace("\\", "/").split("/")[-1]
    cleaned = "".join(c for c in last if not (ord(c) <= 0x1F or ord(c) == 0x7F))
    cleaned = "".join("_" if c in '<>:"|?*' else c for c in cleaned)
    trimmed = cleaned.strip(" .")
    if not trimmed:
        return None
    if trimmed.split(".")[0].upper() in RESERVED:
        trimmed = "_" + trimmed
    if len(trimmed) <= 200:
        return trimmed
    dot = trimmed.rfind(".")
    if dot > 0 and len(trimmed) - dot - 1 <= 20:
        ext = trimmed[dot:]
        return trimmed[:dot][: 200 - len(ext)] + ext
    return trimmed[:200]


def unique_path(directory: Path, name: str) -> Path:
    p = directory / name
    if not p.exists():
        return p
    dot = name.rfind(".")
    stem, ext = (name[:dot], name[dot:]) if dot > 0 else (name, "")
    n = 1
    while (directory / f"{stem} ({n}){ext}").exists():
        n += 1
    return directory / f"{stem} ({n}){ext}"


# ---------------------------------------------------------------- identity
V0_MODE = False  # --v0: behave exactly like a v0 node (no `crypto` fields, no encryption)


def me(name: str | None = None) -> dict:
    return {"id": str(uuid.uuid4()), "name": name or f"hoppy-ref@{socket.gethostname()}"[:64], "platform": "linux"}


# ---------------------------------------------------------------- send
def send(host: str, port: int, paths: list[Path], ident: Identity) -> dict:
    """Direct connection only (spec §3/§7): v1 first, v0 if the receiver is an old node."""
    s = socket.create_connection((host, port), timeout=10)
    return send_session(s, paths, ident, None, None, lambda: socket.create_connection((host, port), timeout=10))


def send_to(host: str, port: int, paths: list[Path], ident: Identity, peer_udp: tuple[str, int],
            udp_port: int = 0, direct: bool = True) -> dict:
    """Direct first (3 s); if the connect fails, ask the receiver to connect back (spec §6)."""
    if direct:
        try:
            s = socket.create_connection((host, port), timeout=DIRECT_CONNECT_TIMEOUT)
        except OSError as e:
            print(f"direct connect failed ({e}); asking {host} to connect back", file=sys.stderr)
        else:
            s.settimeout(10)
            return send_session(s, paths, ident, None, None,
                                lambda: socket.create_connection((host, port), timeout=10))
    cb = callback_connect(peer_udp, ident, udp_port)
    with cb as s:
        advertised = max(cb.hello.get("crypto") or 0, cb.peer_crypto or 0)
        return send_session(s, paths, ident, advertised, cb.peer_id, None)


def send_session(s: socket.socket, paths: list[Path], ident: Identity, advertised: int | None,
                 peer_id: str | None, reconnect) -> dict:
    """Spec §7.1: v1 when the peer advertised it, is pinned (by id or by an address where a
    remembered device encrypted), or is unknown (then v0 if it closes right after our hello
    and `reconnect` is possible); v0 for a known v0 peer."""
    with s:
        deadline = time.monotonic() + SESSION_DEADLINE
        if ident.v0:
            return send_over(s, paths, ident.device, hello=True, deadline=deadline)
        host, port = s.getpeername()[:2]
        addr = f"[{host}]:{port}" if ":" in host else f"{host}:{port}"  # same form as Rust
        pinned = (bool(peer_id) and peer_id in ident.peers) or ident.encrypted_at(addr)
        if advertised == 0 and not pinned and not ident.require_encryption:
            print("sending: NOT encrypted connection", file=sys.stderr)
            return {**send_over(s, paths, ident.device, hello=True, deadline=deadline), "encrypted": False}
        try:
            stream, info = initiate(s, ident, peer_id, deadline=deadline)
        except PeerClosed:
            if advertised is None and reconnect and not pinned and not ident.require_encryption:
                print("sending: NOT encrypted connection", file=sys.stderr)
                with reconnect() as s2:
                    s2.settimeout(10)
                    return {**send_over(s2, paths, ident.device, hello=True, deadline=time.monotonic() + SESSION_DEADLINE),
                            "encrypted": False}
            raise ConnectionError("the receiver did not answer the encrypted handshake"
                                  + (" although it encrypted before" if pinned else ""))
        print(describe(info), file=sys.stderr)
        result = send_over(stream, paths, ident.device, hello=False, deadline=deadline,
                           on_accept=lambda: ident.pin(info["peer"], addr=addr))
        return {**result, "encrypted": True, "sas": info["sas"]["digits"], "trust": info["trust"]}


def send_over(s, paths: list[Path], sender: dict, hello: bool, on_accept=None, deadline: float | None = None) -> dict:
    """Spec §3 after the hello, over a socket (v0) or a NoiseSocket (v1). `deadline` bounds
    the wait for the Decision (handshake + Decision, §7.3)."""
    files = [{"name": p.name, "size": p.stat().st_size} for p in paths]
    if hello:
        s.sendall(MAGIC + bytes([VERSION]))
    write_json(s, {"sender": sender, "files": files})
    s.settimeout(120)
    if isinstance(s, NoiseSocket):
        s.deadline = deadline
        decision = read_json(s)
        s.deadline = None
    else:
        decision = read_json(s, deadline=deadline)
    if not decision.get("accept"):
        return {"declined": decision.get("reason")}
    if on_accept:
        on_accept()
    s.settimeout(30)
    for p in paths:
        h = hashlib.sha256()
        with p.open("rb") as f:
            while chunk := f.read(CHUNK):
                h.update(chunk)
                s.sendall(chunk)
        s.sendall(h.digest())
    return read_json(s)


def abort_handshakes(host: str, port: int, count: int) -> dict:
    """Spec §7.3 test helper: handshakes abandoned right after message 2."""
    served = refused = 0
    for _ in range(count):
        with socket.create_connection((host, port), timeout=10) as s:
            hs = NoiseXX(True, os.urandom(32))
            s.sendall(PROLOGUE + frame(hs.write_msg1(json.dumps({"commit": "00" * 32}).encode())))
            msg2 = read_frame(s, MAX_HANDSHAKE_MESSAGE)
            if msg2:
                served += 1
            elif msg2 == b"":
                refused += 1
        time.sleep(0.05)
    return {"served": served, "refused": refused}


def verify(host: str, port: int, payload: str, ident: Identity) -> dict:
    """Scanner side of spec §7.7: open a v1 session that must present the scanned key, then
    send the one-time code."""
    qr = parse_qr(payload)
    if qr is None:
        return {"ok": False, "error": "not a Hoppy verification code"}
    with socket.create_connection((host, port), timeout=10) as s:
        stream, info = initiate(s, ident, qr["id"], expect_fp=qr["fingerprint"])
        # The scanned key matched the session's: verified here, before telling the other side.
        ident.pin(info["peer"], verified=True)
        write_json(stream, {"verify": qr["code"]})
        return read_json(stream)


# ---------------------------------------------------------------- callback (spec §6)
def valid_token(t) -> bool:
    return isinstance(t, str) and len(t) == 32 and all(c in "0123456789abcdef" for c in t)


def is_local_address(ip: str) -> bool:
    a = ipaddress.ip_address(ip)
    if isinstance(a, ipaddress.IPv6Address) and a.ipv4_mapped:
        a = a.ipv4_mapped
    if isinstance(a, ipaddress.IPv4Address):
        return a.is_loopback or a.is_link_local or any(
            a in ipaddress.ip_network(n) for n in ("10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16", "100.64.0.0/10"))
    return a.is_loopback or any(a in ipaddress.ip_network(n) for n in ("fc00::/7", "fe80::/10"))


def callback_hello(receiver_id: str, token: str, crypto: bool | None = None) -> bytes:
    """`HOPC 0x00` + {"id","token"} (+ "crypto":1 from v1 receivers, spec §7.1)."""
    crypto = (not V0_MODE) if crypto is None else crypto
    obj = {"id": receiver_id, "token": token, **({"crypto": CRYPTO_VERSION} if crypto else {})}
    body = json.dumps(obj, separators=(",", ":"), ensure_ascii=False).encode()
    return CALLBACK_MAGIC + bytes([VERSION]) + struct.pack(">I", len(body)) + body


class CallbackLimiter:
    """Receiver-side limits (spec §6): 1 outstanding per IP, 10/min per IP, 30/min in total;
    a token is remembered for 2 min only once it connected back (mark_used)."""

    def __init__(self) -> None:
        self.lock = threading.Lock()
        self.used: dict[str, float] = {}
        self.outstanding: set[str] = set()
        self.started: list[tuple[str, float]] = []

    def admit(self, ip: str, token: str, now: float | None = None) -> bool:
        now = time.monotonic() if now is None else now
        with self.lock:
            self.used = {t: at for t, at in self.used.items() if now - at <= TOKEN_MEMORY}
            self.started = [(i, t) for i, t in self.started if now - t < 60]
            from_ip = sum(1 for i, _ in self.started if i == ip)
            if (token in self.used or ip in self.outstanding or from_ip >= MAX_CALLBACKS_PER_MINUTE
                    or len(self.started) >= MAX_CALLBACKS_PER_MINUTE_TOTAL):
                return False
            self.outstanding.add(ip)
            self.started.append((ip, now))
            return True

    def mark_used(self, token: str, now: float | None = None) -> None:
        with self.lock:
            self.used[token] = time.monotonic() if now is None else now

    def release(self, ip: str) -> None:
        with self.lock:
            self.outstanding.discard(ip)


def answer_callback(ip: str, port: int, token: str, ident: Identity, out: Path, limiter: CallbackLimiter) -> None:
    try:
        conn = socket.create_connection((ip, port), timeout=CALLBACK_CONNECT_TIMEOUT)
        conn.sendall(callback_hello(ident.device["id"], token, crypto=not ident.v0))
        limiter.mark_used(token)  # only now: a re-send after a failed connect may retry
        handle(conn, out, ident)
    except (OSError, ValueError) as e:
        print(f"callback to {ip}:{port} failed: {e}", file=sys.stderr)
    finally:
        limiter.release(ip)


class callback_connect:
    """Sender side: context manager yielding the verified socket the receiver opened.
    Afterwards `hello` is its callback hello, `peer_id`/`peer_crypto` what it announced."""

    def __init__(self, peer_udp: tuple[str, int], ident: Identity, udp_port: int = 0) -> None:
        self.peer_udp, self.ident, self.udp_port = peer_udp, ident, udp_port
        self.conn: socket.socket | None = None
        self.hello: dict = {}
        self.peer_id: str | None = None
        self.peer_crypto: int | None = None

    def __enter__(self) -> socket.socket:
        udp = udp_socket(self.udp_port)
        srv = socket.create_server(("0.0.0.0", 0))
        try:
            self.conn = self._run(udp, srv)
            return self.conn
        finally:
            udp.close()
            srv.close()

    def __exit__(self, *exc) -> None:
        if self.conn:
            self.conn.close()

    def _run(self, udp: socket.socket, srv: socket.socket) -> socket.socket:
        token = os.urandom(16).hex()
        port = srv.getsockname()[1]
        sender = self.ident.device
        # Learn the receiver's id (needed for "to") from its answer to a unicast query.
        peer_ip = socket.gethostbyname(self.peer_udp[0])
        target = (peer_ip, self.peer_udp[1])
        udp.sendto(msg("query", sender, 0), target)
        end = time.monotonic() + CALLBACK_WINDOW
        next_resend = 0.0
        while time.monotonic() < end:
            now = time.monotonic()
            if self.peer_id and now >= next_resend:
                udp.sendto(msg("callback", sender, port, token=token, to=self.peer_id), target)
                next_resend = now + CALLBACK_RESEND
            ready, _, _ = select.select([udp, srv], [], [], 0.2)
            if udp in ready:
                data, addr = udp.recvfrom(2048)
                m = decode(data)
                # Only the receiver's known IP may speak for it (spec §6 sender step 2).
                if (m and m["type"] in ("announce", "query") and m["port"] and addr[0] == peer_ip
                        and is_local_address(addr[0])):
                    if self.peer_id is None:
                        self.peer_id = m["id"]
                    if m["id"] == self.peer_id:  # reply inside the firewall's response window
                        self.peer_crypto = m.get("crypto") if isinstance(m.get("crypto"), int) else 0
                        target = addr
                        udp.sendto(msg("callback", sender, port, token=token, to=self.peer_id), addr)
                        next_resend = time.monotonic() + CALLBACK_RESEND
            if srv in ready:
                conn, (from_ip, *_) = srv.accept()
                if from_ip != peer_ip:  # spec §6 step 3: closed before reading anything
                    conn.close()
                    continue
                try:
                    deadline = time.monotonic() + CALLBACK_HELLO_TIMEOUT
                    if read_exact(conn, 5, deadline) != CALLBACK_MAGIC + bytes([VERSION]):
                        raise ValueError("bad callback magic")
                    hello = read_json(conn, MAX_CALLBACK_HELLO_FRAME, deadline)
                    if hmac.compare_digest(str(hello.get("token", "")).encode(), token.encode()) and hello.get("id") == self.peer_id:
                        conn.settimeout(10)
                        self.hello = hello if isinstance(hello.get("crypto", 0), int) else {}
                        return conn
                    raise ValueError("callback token mismatch")
                except (OSError, ValueError):
                    conn.close()
        raise ConnectionError(f"{self.peer_udp[0]} did not connect directly or back within {CALLBACK_WINDOW:.0f} s")


# ---------------------------------------------------------------- receive
def handle(conn: socket.socket, out: Path, ident: Identity, log=print) -> None:
    with conn:
        conn.settimeout(30)
        hello = read_exact(conn, 5)
        if hello[:4] != MAGIC:
            return
        if hello[4] == VERSION_V1 and not ident.v0:
            try:
                stream, info, first = respond(conn, ident)
            except Refused as e:
                log(f"WARNING: {e}")
                return
        elif hello[4] == VERSION:
            stream, info = conn, {"encrypted": False}
            first = read_json(stream)
        else:
            return  # a v0 node closes on any other version (the sender then falls back)
        if "verify" in first:  # spec §7.7: only inside an encrypted session
            if not info["encrypted"]:
                return
            peer = info["peer"]
            if info["trust"] == "key_changed":
                write_json(stream, {"ok": False, "error": "key_changed"})
            elif ident.take_qr_code(first["verify"]):
                ident.pin(peer, verified=True)
                write_json(stream, {"ok": True})
                log(f"verified {peer['name']} ({peer['fingerprint'][:8]}…) by QR")
            else:
                write_json(stream, {"ok": False, "error": "invalid_code"})
            return
        offer = first
        files = offer.get("files", [])
        sender = offer.get("sender") if isinstance(offer.get("sender"), dict) else {}
        names = [sanitize(f["name"]) for f in files]
        if not info["encrypted"] and ident.require_encryption:
            write_json(stream, {"accept": False, "reason": "encryption_required"})
            return
        if not info["encrypted"] and sender.get("id") in ident.peers:
            write_json(stream, {"accept": False, "reason": "declined"})  # specific reason stays local
            log(f"refused plaintext offer from {sender.get('name')}: that device encrypts")
            return
        if info["encrypted"] and sender.get("id") != info["peer"]["id"]:
            write_json(stream, {"accept": False, "reason": "invalid_offer"})
            return
        if info["encrypted"] and info["trust"] == "key_changed":
            write_json(stream, {"accept": False, "reason": "declined"})  # specific reason stays local
            log(f"WARNING: the key of {info['peer']['name']} has changed: offer refused")
            return
        if not files or len(files) > MAX_FILES or None in names:
            write_json(stream, {"accept": False, "reason": "too_many_files" if len(files) > MAX_FILES else "invalid_offer"})
            return
        log(f"offer from {sender.get('name')}: {describe(info)}")
        # This tool accepts automatically, so it never remembers keys on accept (TOFU needs a
        # person's yes, spec §7.5); keys are remembered when it sends or verifies.
        write_json(stream, {"accept": True})
        out.mkdir(parents=True, exist_ok=True)
        failed = []
        for f, name in zip(files, names):
            part = unique_path(out, name + ".hoppy-part")
            h = hashlib.sha256()
            remaining = int(f["size"])
            try:
                with part.open("xb") as w:
                    while remaining:
                        chunk = stream.recv(min(CHUNK, remaining))
                        if not chunk:
                            raise ConnectionError("closed mid-file")
                        h.update(chunk)
                        w.write(chunk)
                        remaining -= len(chunk)
                if read_exact(stream, 32) == h.digest():
                    part.rename(unique_path(out, name))
                else:
                    failed.append(name)
                    part.unlink()
            except BaseException:
                part.unlink(missing_ok=True)
                raise
        result = {"ok": True} if not failed else {"ok": False, "error": "hash mismatch: " + ", ".join(failed)}
        write_json(stream, result)
        log(f"received {len(files) - len(failed)}/{len(files)} file(s) from {sender.get('name')}")


def _handle_logged(conn: socket.socket, out: Path, ident: Identity) -> None:
    try:
        handle(conn, out, ident)
    except (OSError, ValueError, KeyError) as e:
        print(f"connection failed: {e}", file=sys.stderr)


def receive(out: Path, port: int, ident: Identity, announce: bool = True, udp_port: int = DISCOVERY_PORT,
            show_qr: bool = False) -> None:
    srv = socket.create_server(("0.0.0.0", port), reuse_port=hasattr(socket, "SO_REUSEPORT"))
    port = srv.getsockname()[1]
    if announce:
        threading.Thread(target=announcer, args=(ident, port, out, udp_port), daemon=True).start()
    print(f"receiving as {ident.device['name']!r} on port {port} into {out}", flush=True)
    if not ident.v0:
        print(f"fingerprint {ident.fingerprint}", flush=True)
    if show_qr:
        print(f"qr {ident.begin_qr()}", flush=True)
    while True:
        conn, _ = srv.accept()
        threading.Thread(target=_handle_logged, args=(conn, out, ident), daemon=True).start()


# ---------------------------------------------------------------- discovery
def msg(kind: str, device: dict, port: int, **extra: str) -> bytes:
    crypto = {} if V0_MODE else {"crypto": CRYPTO_VERSION}
    return json.dumps({"hoppy": VERSION, "type": kind, **device, "port": port, **extra, **crypto},
                      separators=(",", ":"), ensure_ascii=False).encode()


def decode(data: bytes) -> dict | None:
    """Valid v0 discovery datagram or None (spec §2 rule 5, §6)."""
    try:
        m = json.loads(data)
    except ValueError:
        return None
    if not isinstance(m, dict) or m.get("hoppy") != VERSION or m.get("type") not in ("announce", "query", "bye", "callback"):
        return None
    port, ident = m.get("port"), m.get("id")
    if not isinstance(port, int) or isinstance(port, bool) or not 0 <= port <= 65535 or not isinstance(ident, str) or not ident:
        return None
    if m["type"] == "callback":
        to = m.get("to")
        if port == 0 or not valid_token(m.get("token")) or not isinstance(to, str) or not to:
            return None
    return m


def udp_socket(bind_port: int) -> socket.socket:
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM, socket.IPPROTO_UDP)
    s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    if hasattr(socket, "SO_REUSEPORT"):
        s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEPORT, 1)
    s.setsockopt(socket.SOL_SOCKET, socket.SO_BROADCAST, 1)
    s.bind(("0.0.0.0", bind_port))
    if bind_port == DISCOVERY_PORT:
        mreq = struct.pack("4s4s", socket.inet_aton(MCAST), socket.inet_aton("0.0.0.0"))
        s.setsockopt(socket.IPPROTO_IP, socket.IP_ADD_MEMBERSHIP, mreq)
    return s


def broadcast(s: socket.socket, payload: bytes) -> None:
    for target in (MCAST, "255.255.255.255"):
        try:
            s.sendto(payload, (target, DISCOVERY_PORT))
        except OSError:
            pass


def announcer(ident: Identity, port: int, out: Path | None = None, udp_port: int = DISCOVERY_PORT) -> None:
    device = ident.device
    s = udp_socket(udp_port)
    s.settimeout(0.5)
    limiter = CallbackLimiter()
    broadcast(s, msg("query", device, port))
    next_announce = 0.0
    while True:
        if time.monotonic() >= next_announce:
            broadcast(s, msg("announce", device, port))
            next_announce = time.monotonic() + 5
        try:
            data, addr = s.recvfrom(2048)
        except socket.timeout:
            continue
        m = decode(data)
        if not m or m["id"] == device["id"]:
            continue
        if m["type"] == "query":
            s.sendto(msg("announce", device, port), addr)
        elif (m["type"] == "callback" and out is not None and port and m["to"] == device["id"]
              and is_local_address(addr[0]) and limiter.admit(addr[0], m["token"])):
            # Connect to the datagram's source IP, never to an address from the JSON.
            threading.Thread(target=answer_callback, daemon=True,
                             args=(addr[0], m["port"], m["token"], ident, out, limiter)).start()


def peers(wait: float, seeds: list[str]) -> dict:
    device = me()
    s = udp_socket(0)  # send-only node (spec §2 rule 6)
    s.settimeout(0.2)
    payload = msg("query", device, 0)
    broadcast(s, payload)
    for seed in seeds:
        host, _, p = seed.partition(":")
        s.sendto(payload, (host, int(p or DISCOVERY_PORT)))
    found: dict = {}
    end = time.monotonic() + wait
    while time.monotonic() < end:
        try:
            data, addr = s.recvfrom(2048)
        except socket.timeout:
            continue
        m = decode(data)
        if m and m["type"] in ("announce", "query") and m["port"]:
            found[m["id"]] = {**m, "ip": addr[0]}
    return found


# ---------------------------------------------------------------- selftest
def selftest() -> int:
    vectors = json.loads((ROOT / "docs" / "test-vectors.json").read_text())
    bad = 0
    for case in vectors["sanitize"]:
        got = sanitize(case["input"])
        if got != case["output"]:
            bad += 1
            print(f"FAIL sanitize({case['input']!r}) = {got!r}, want {case['output']!r}")
    ex = vectors["offer_example"]
    for name, content in ex["files_content_utf8"].items():
        if hashlib.sha256(content.encode()).hexdigest() != ex["sha256_hex"][name]:
            bad += 1
            print(f"FAIL sha256 vector for {name}")
    cb = vectors["callback_example"]
    m = decode(cb["datagram_json"].encode())
    if not m or m["type"] != "callback":
        bad += 1
        print("FAIL callback datagram vector rejected")
    else:
        dev = {k: m[k] for k in ("id", "name", "platform")}
        global V0_MODE
        V0_MODE, saved = True, V0_MODE
        if msg("callback", dev, m["port"], token=m["token"], to=m["to"]).decode() != cb["datagram_json"]:
            bad += 1
            print("FAIL callback datagram encoding")
        V0_MODE = saved
    for raw in cb["invalid_datagrams"]:
        if decode(raw.encode()) is not None:
            bad += 1
            print(f"FAIL accepted invalid callback datagram {raw}")
    h = json.loads(cb["hello_json"])
    if callback_hello(h["id"], h["token"], crypto=False).hex() != cb["hello_hex"]:
        bad += 1
        print("FAIL callback hello bytes")
    if decode(msg("callback", me(), 1, token="ab" * 16, to="x")) is None or valid_token("AB" * 16):
        bad += 1
        print("FAIL token validation")
    lim = CallbackLimiter()
    a = "10.0.0.1"
    checks = [lim.admit(a, "t1", 0), not lim.admit(a, "t2", 0)]
    lim.release(a)
    checks += [lim.admit(a, "t1", 0)]  # failed connect: same token may retry
    lim.mark_used("t1", 0)
    lim.release(a)
    checks += [not lim.admit(a, "t1", 0)]  # connected back: re-sends ignored
    for i in range(MAX_CALLBACKS_PER_MINUTE - 2):
        checks.append(lim.admit(a, f"t{i + 3}", 0))
        lim.release(a)
    checks += [not lim.admit(a, "fresh", 0)]  # per-IP cap
    checks += [lim.admit(f"10.0.1.{i}", f"u{i}", 0)
               for i in range(MAX_CALLBACKS_PER_MINUTE_TOTAL - MAX_CALLBACKS_PER_MINUTE)]
    checks += [not lim.admit("10.0.2.1", "v", 0), lim.admit("10.0.2.1", "v", 61)]  # global cap
    checks += [not lim.admit("10.0.2.2", "t1", 61), lim.admit("10.0.2.2", "t1", 181)]
    checks += [callback_listener_checks()]
    checks += [is_local_address(a) for a in ("127.0.0.1", "192.168.1.2", "100.64.0.1", "fe80::1", "fd00::1")]
    checks += [not is_local_address(a) for a in ("8.8.8.8", "100.128.0.1", "2001:db8::1")]
    if not all(checks):
        bad += 1
        print(f"FAIL callback limits/addresses: {checks}")
    bad += rfc_checks()
    bad += v1_checks(vectors["v1"])
    print("selftest:", "OK" if not bad else f"{bad} failure(s)")
    return 1 if bad else 0


def rfc_checks() -> int:
    """Known-answer tests of the primitives (RFC 7748 §5.2/§6.1, RFC 8439 §2.5.2/§2.8.2,
    RFC 4231 case 2, RFC 5869 case 1)."""
    bad = 0

    def check(label: str, got: bytes, want_hex: str) -> None:
        nonlocal bad
        if got.hex() != want_hex:
            bad += 1
            print(f"FAIL {label}: {got.hex()}")

    check("x25519 rfc7748 5.2", x25519(
        bytes.fromhex("a546e36bf0527c9d3b16154b82465edd62144c0ac1fc5a18506a2244ba449ac4"),
        bytes.fromhex("e6db6867583030db3594c1a424b15f7c726624ec26b3353b10a903a6d0ab1c4c")),
        "c3da55379de9c6908e94ea4df28d084f32eccf03491c71f754b4075577a28552")
    alice = bytes.fromhex("77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a")
    bob = bytes.fromhex("5dab087e624a8a4b79e17f8b83800ee66f3bb1292618b6fd1c2f8b27ff88e0eb")
    check("x25519 alice pub", x25519_public(alice), "8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a")
    check("x25519 shared", x25519(alice, x25519_public(bob)),
          "4a5d9d5ba4ce2de1728e3bf480350f25e07e21c947d19e3376f09b3c1e161742")
    check("poly1305 rfc8439 2.5.2", poly1305(
        bytes.fromhex("85d6be7857556d337f4452fe42d506a80103808afb0db2fd4abff6af4149f51b"),
        b"Cryptographic Forum Research Group"), "a8061dc1305136c6c22b8baf0c0127a9")
    key = bytes(range(0x80, 0xA0))
    nonce = bytes.fromhex("070000004041424344454647")
    aad = bytes.fromhex("50515253c0c1c2c3c4c5c6c7")
    pt = (b"Ladies and Gentlemen of the class of '99: If I could offer you only one tip for the future, "
          b"sunscreen would be it.")
    sealed = aead_encrypt(key, nonce, aad, pt)
    check("aead rfc8439 2.8.2 tag", sealed[-16:], "1ae10b594f09e26a7e902ecbd0600691")
    check("aead rfc8439 2.8.2 ct[:16]", sealed[:16], "d31a8d34648e60db7b86afbc53ef7ec2")
    if aead_decrypt(key, nonce, aad, sealed) != pt:
        bad += 1
        print("FAIL aead roundtrip")
    try:
        aead_decrypt(key, nonce, aad, sealed[:-1] + bytes([sealed[-1] ^ 1]))
        bad += 1
        print("FAIL aead accepted a bad tag")
    except ValueError:
        pass
    check("hmac rfc4231 case 2", hmac_sha256(b"Jefe", b"what do ya want for nothing?"),
          "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843")
    check("hkdf rfc5869 case 1", hkdf_rfc5869(bytes(range(13)), b"\x0b" * 22, bytes(range(0xF0, 0xFA)), 42),
          "3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865")
    check("base32 rfc4648", base32(b"foobar").encode(), b"MZXW6YTBOI".hex())
    return bad


def v1_checks(want: dict) -> int:
    """Regenerates the v1 vectors and compares them with docs/test-vectors.json, then runs
    an in-memory session and a few parser checks."""
    bad = 0
    got = v1_vectors()
    for key in got:
        if got[key] != want.get(key):
            bad += 1
            print(f"FAIL v1 vector {key!r} differs (python3 tools/hoppy_ref.py vectors --v1)")
    for i, row in enumerate(want["sas_emoji"]):
        e = row["emoji"]
        if len(e) != 1 or row["codepoints"] != f"U+{ord(e):04X}" or not unicodedata.name(e, ""):
            bad += 1
            print(f"FAIL emoji row {i}: must be one named code point")
    if len({r["emoji"] for r in want["sas_emoji"]}) != 64 or len({r["it"] for r in want["sas_emoji"]}) != 64 \
            or len({r["en"] for r in want["sas_emoji"]}) != 64:
        bad += 1
        print("FAIL emoji table: 64 distinct emoji and names")
    neg = want["negative"]
    hs_v, keys = want["handshake"], want["keys"]

    def fresh() -> tuple[NoiseXX, NoiseXX]:
        i = NoiseXX(True, bytes.fromhex(keys["initiator"]["static_private"]),
                    bytes.fromhex(keys["initiator"]["ephemeral_private"]))
        r = NoiseXX(False, bytes.fromhex(keys["responder"]["static_private"]),
                    bytes.fromhex(keys["responder"]["ephemeral_private"]))
        r.read_msg1(i.write_msg1(hs_v["msg1_payload"].encode()))
        return i, r

    def rejects(label: str, fn) -> None:
        nonlocal bad
        try:
            fn()
        except ValueError:
            return
        bad += 1
        print(f"FAIL accepted negative vector {label}")

    def bad_commit() -> None:
        i, r = fresh()
        i.read_msg2(r.write_msg2(hs_v["msg2_payload"].encode()))
        _, n = parse_identity(r.read_msg3(bytes.fromhex(neg["msg3_bad_commitment_hex"])))
        their = bytes.fromhex(json.loads(hs_v["msg1_payload"])["commit"])
        if not hmac.compare_digest(commit(n), their):
            raise ValueError("SAS commitment mismatch")

    def tampered3() -> None:
        i, r = fresh()
        i.read_msg2(r.write_msg2(hs_v["msg2_payload"].encode()))
        r.read_msg3(bytes.fromhex(neg["msg3_tampered_hex"]))

    def tampered2() -> None:
        i, r = fresh()
        r.write_msg2(hs_v["msg2_payload"].encode())
        i.read_msg2(bytes.fromhex(neg["msg2_tampered_hex"]))

    class Buf:
        def __init__(self, b: bytes) -> None:
            self.b = b

        def recv(self, n: int) -> bytes:
            out, self.b = self.b[:n], self.b[n:]
            return out

        def settimeout(self, t) -> None:
            pass

    rejects("msg1_low_order", lambda: NoiseXX(False, os.urandom(32)).read_msg1(bytes.fromhex(neg["msg1_low_order_hex"])))
    rejects("msg2_tampered", tampered2)
    rejects("msg3_tampered", tampered3)
    rejects("msg3_bad_commitment", bad_commit)
    rejects("frame_too_long", lambda: read_frame(Buf(bytes.fromhex(neg["handshake_frame_too_long_hex"]) + b"x" * 2049),
                                                 MAX_HANDSHAKE_MESSAGE))
    for name in ("transport_empty_hex", "transport_short_hex"):
        ns = NoiseSocket(Buf(bytes.fromhex(neg[name])), CipherState(), CipherState(b"\x00" * 32))
        rejects(name, lambda: ns.recv(1))
    for pt in neg["low_order_points"]:
        if not is_low_order(bytes.fromhex(pt)):
            bad += 1
            print(f"FAIL low-order point accepted {pt}")
    if is_low_order(bytes.fromhex(keys["initiator"]["static_public"])):
        bad += 1
        print("FAIL a valid key was called low-order")
    g = HandshakeGuard()
    checks = [g.admit("a", 0)] + [g.record_abort("a", 0) > 0 for _ in range(MAX_ABORTS_PER_IP)]
    checks += [not g.admit("a", 1), g.admit("b", 1), not g.admit("a", 59), g.admit("a", 61)]
    for k in range(MAX_ABORTS_TOTAL):
        g.record_abort(f"10.0.0.{k}", 100)
    checks += [not g.admit("c", 100), g.admit("c", 161)]
    if not all(checks):
        bad += 1
        print(f"FAIL handshake guard: {checks}")
    if display_text("a\u202eb\u200d\x07 ", 10) != "ab\u200d":
        bad += 1
        print("FAIL display_text")
    rejects("id with bidi", lambda: parse_identity(b'{"id":"x\\u202e","name":"n","platform":"p","nonce":"' + b"00" * 32 + b'"}'))
    rejects("non-string name", lambda: parse_identity(b'{"id":"x","name":1,"platform":"p","nonce":"' + b"00" * 32 + b'"}'))
    qr = want["qr_example"]
    if parse_qr(qr["payload"]) != {"id": qr["id"], "fingerprint": qr["fingerprint"], "code": qr["code"]} \
            or any(parse_qr(x) is not None for x in qr["invalid"]):
        bad += 1
        print("FAIL QR payload parsing")
    # Tampering and replay are rejected by the transport.
    a = NoiseXX(True, os.urandom(32))
    b = NoiseXX(False, os.urandom(32))
    b.read_msg1(a.write_msg1(b"{}"))
    a.read_msg2(b.write_msg2(b"{}"))
    b.read_msg3(a.write_msg3(b"{}"))
    (atx, _), (_, brx) = a.split(), b.split()
    c1 = atx.encrypt(b"", b"hello")
    for bad_msg in (c1[:-1] + bytes([c1[-1] ^ 1]),):
        try:
            CipherState(brx.k).decrypt(b"", bad_msg)
            bad += 1
            print("FAIL tampered transport message accepted")
        except ValueError:
            pass
    if brx.decrypt(b"", c1) != b"hello":
        bad += 1
        print("FAIL transport roundtrip")
    try:
        brx.decrypt(b"", c1)  # replay: counter moved on
        bad += 1
        print("FAIL replayed transport message accepted")
    except ValueError:
        pass
    return bad


def _vec_secret(label: str) -> bytes:
    return hashlib.sha256(f"hoppy-v1-vector {label}".encode()).digest()


def v1_vectors() -> dict:
    """Deterministic v1 vectors (spec §7): fixed keys/nonces → exact bytes."""
    dev_i = {"id": "00000000-0000-4000-8000-000000000001", "name": "Phone", "platform": "android"}
    dev_r = {"id": "00000000-0000-4000-8000-000000000002", "name": "PC", "platform": "windows"}
    keys = {}
    for who, dev in (("initiator", dev_i), ("responder", dev_r)):
        s = _vec_secret(f"{who} static")
        keys[who] = {"device": dev, "static_private": s.hex(), "static_public": x25519_public(s).hex(),
                     "fingerprint": fingerprint(x25519_public(s)),
                     "ephemeral_private": _vec_secret(f"{who} ephemeral").hex(),
                     "sas_nonce": _vec_secret(f"{who} nonce").hex()}
    ki, kr = keys["initiator"], keys["responder"]
    i = NoiseXX(True, bytes.fromhex(ki["static_private"]), bytes.fromhex(ki["ephemeral_private"]))
    r = NoiseXX(False, bytes.fromhex(kr["static_private"]), bytes.fromhex(kr["ephemeral_private"]))
    n_i, n_r = bytes.fromhex(ki["sas_nonce"]), bytes.fromhex(kr["sas_nonce"])
    p1 = json.dumps({"commit": commit(n_i).hex()}, separators=(",", ":")).encode()
    p2, p3 = identity_payload(dev_r, n_r), identity_payload(dev_i, n_i)
    m1 = i.write_msg1(p1)
    r.read_msg1(m1)
    m2 = r.write_msg2(p2)
    h2 = r.h
    i.read_msg2(m2)
    m3 = i.write_msg3(p3)
    r.read_msg3(m3)
    (itx, irx), (rtx, rrx) = i.split(), r.split()
    offer = json.dumps({"sender": dev_i, "files": [{"name": "a.txt", "size": 5}]}, separators=(",", ":")).encode()
    accept = b'{"accept":true}'
    transport = []
    for frm, cs, body in (("initiator", itx, offer), ("responder", rtx, accept)):
        plain = struct.pack(">I", len(body)) + body
        transport.append({"from": frm, "nonce": 0, "plaintext_hex": plain.hex(),
                          "message_hex": frame(cs.encrypt(b"", plain)).hex()})
    sas = sas_render(sas_value(h2, n_i, n_r))
    # Negative cases: a responder/initiator in the state of the positive run must reject these.
    other = NoiseXX(True, bytes.fromhex(ki["static_private"]), bytes.fromhex(ki["ephemeral_private"]))
    other_r = NoiseXX(False, bytes.fromhex(kr["static_private"]), bytes.fromhex(kr["ephemeral_private"]))
    other_r.read_msg1(other.write_msg1(p1))
    other.read_msg2(other_r.write_msg2(p2))
    bad_commit_msg3 = other.write_msg3(identity_payload(dev_i, _vec_secret("initiator other nonce")))
    flip = lambda m, i: m[:i] + bytes([m[i] ^ 1]) + m[i + 1:]  # noqa: E731
    p25519 = P25519.to_bytes(32, "little")
    negative = {
        "_comment": "Each must be rejected. msg2/msg3 cases: by the side that reads them, in the state of the "
                    "positive handshake above (same keys and payloads). frame cases: by any reader.",
        "msg1_low_order_hex": ("00" * 32) + p1.hex(),
        "msg2_tampered_hex": flip(m2, len(m2) - 1).hex(),
        "msg3_tampered_hex": flip(m3, 40).hex(),
        "msg3_bad_commitment_hex": bad_commit_msg3.hex(),
        "handshake_frame_too_long_hex": struct.pack(">H", MAX_HANDSHAKE_MESSAGE + 1).hex(),
        "transport_empty_hex": frame(CipherState(_hkdf2(i.ck, b"")[0]).encrypt(b"", b"")).hex(),
        "transport_short_hex": "000501020304 05".replace(" ", ""),
        "low_order_points": [
            "00" * 32, "01" + "00" * 31,
            "e0eb7a7c3b41b8ae1656e3faf19fc46ada098deb9c32b1fd866205165f49b800",
            "5f9c95bca3508c24b1d0b1559c83ef5b04445cc4581c8e86d8224eddd09f1157",
            (int.from_bytes(p25519, "little") - 1).to_bytes(32, "little").hex(), p25519.hex(),
            (int.from_bytes(p25519, "little") + 1).to_bytes(32, "little").hex(),
        ],
        "busy_reply_hex": "0000",
    }
    code = "00112233445566778899aabbccddeeff"
    qr = qr_payload(dev_r["id"], kr["fingerprint"], code)
    hello_json = json.dumps({"id": dev_r["id"], "token": code, "crypto": 1}, separators=(",", ":"))
    return {
        "_comment": "Spec §7 (protocol v1). Generated by `python3 tools/hoppy_ref.py vectors --v1`; Rust "
                    "(crypto.rs, session.rs, protocol.rs) and the Python selftest check every field. Private "
                    "keys here are test-only values derived from fixed labels.",
        "keys": keys,
        "handshake": {
            "protocol_name": NOISE_NAME.decode(), "prologue_hex": PROLOGUE.hex(),
            "msg1_payload": p1.decode(), "msg2_payload": p2.decode(), "msg3_payload": p3.decode(),
            "msg1_hex": m1.hex(), "msg2_hex": m2.hex(), "msg3_hex": m3.hex(),
            "h2_hex": h2.hex(), "h_hex": i.h.hex(),
        },
        "sas": {k: sas[k] for k in ("value", "emoji", "text", "digits", "it", "en")},
        "transport": transport,
        "sas_values": [{k: sas_render(v)[k] for k in ("value", "emoji", "text", "digits")}
                       for v in (0, 1, 63, 64, 0x123456, 9_999_999, 0xFFFFFF)],
        "qr_example": {
            "id": dev_r["id"], "fingerprint": kr["fingerprint"], "code": code, "payload": qr,
            "invalid": [qr.replace("hoppy:verify:1:", "hoppy:verify:2:"), qr[:-1], qr.upper(),
                        qr.replace(kr["fingerprint"], kr["fingerprint"][:-1]),
                        qr.replace(":" + code, ":" + code.upper()),
                        "hoppy:verify:1::" + kr["fingerprint"] + ":" + code],
        },
        "discovery": {
            "announce_json": json.dumps({"hoppy": 0, "type": "announce", **dev_r, "port": 47471, "crypto": 1},
                                        separators=(",", ":")),
            "callback_json": json.dumps({"hoppy": 0, "type": "callback", **dev_i, "port": 50123, "token": code,
                                         "to": dev_r["id"], "crypto": 1}, separators=(",", ":")),
            "callback_hello_json": hello_json,
            "callback_hello_hex": (CALLBACK_MAGIC + b"\x00" + struct.pack(">I", len(hello_json))
                                   + hello_json.encode()).hex(),
        },
        "negative": negative,
        "sas_emoji": [{"index": n, "emoji": e, "codepoints": f"U+{ord(e):04X}", "it": it, "en": en}
                      for n, (e, it, en) in enumerate(SAS_EMOJI)],
    }


def callback_listener_checks() -> bool:
    """Sender side of spec §6 step 3 on loopback: oversized, non-object and slow hellos are
    closed cleanly; a connection from another IP (127.0.0.2) is closed unread."""
    sender = Identity(me("selftest-tx"))
    rx_udp = udp_socket(0)
    rx_udp.settimeout(3)
    cb = callback_connect(("127.0.0.1", rx_udp.getsockname()[1]), sender)
    result: dict = {}
    t = threading.Thread(target=lambda: result.update(conn=_try(cb.__enter__)), daemon=True)
    t.start()
    rx = me("selftest-rx")
    ok = True
    try:
        _, s_addr = rx_udp.recvfrom(2048)  # the sender's unicast query
        rx_udp.sendto(msg("announce", rx, 1), s_addr)
        while True:
            m = decode(rx_udp.recvfrom(2048)[0])
            if m and m["type"] == "callback":
                break
        target = ("127.0.0.1", m["port"])

        def closed_quickly(payload: bytes, src: str = "127.0.0.1", within: float = 1.5) -> bool:
            c = socket.socket()
            try:
                c.bind((src, 0))
            except OSError:
                return True  # no 127.0.0.2 here (macOS): skip
            c.connect(target)
            c.sendall(payload)
            c.settimeout(5)
            t0 = time.monotonic()
            try:
                rest = c.recv(1)
            except OSError:
                rest = b""
            c.close()
            return rest == b"" and time.monotonic() - t0 < within

        hdr = CALLBACK_MAGIC + bytes([VERSION])
        ok &= closed_quickly(hdr + struct.pack(">I", MAX_CALLBACK_HELLO_FRAME + 1))
        ok &= closed_quickly(hdr + struct.pack(">I", 5) + b"[1,2]")
        ok &= closed_quickly(hdr + struct.pack(">I", 3) + b'"x"')
        ok &= closed_quickly(callback_hello(rx["id"], m["token"]), src="127.0.0.2")
        ok &= closed_quickly(hdr + b"\0\0", within=CALLBACK_HELLO_TIMEOUT + 1.5)  # stalls
        good = socket.create_connection(target)
        good.sendall(callback_hello(rx["id"], m["token"]))
        t.join(5)
        ok &= isinstance(result.get("conn"), socket.socket) and cb.hello.get("crypto") == CRYPTO_VERSION
        good.close()
    except (OSError, ValueError) as e:
        print(f"callback listener check: {e}")
        ok = False
    finally:
        rx_udp.close()
        cb.__exit__(None, None, None)
    return ok


def _try(fn):
    try:
        return fn()
    except (OSError, ConnectionError) as e:
        return e


def vectors(v1: bool = False) -> None:
    if v1:
        print(json.dumps(v1_vectors(), indent=2, ensure_ascii=False))
        return
    ex = json.loads((ROOT / "docs" / "test-vectors.json").read_text())["offer_example"]
    body = ex["offer_json"].encode()
    print("hello  ", (MAGIC + bytes([VERSION])).hex(" ").upper())
    print("length ", struct.pack(">I", len(body)).hex(" ").upper(), f"({len(body)} bytes of JSON)")
    for name, content in ex["files_content_utf8"].items():
        print("content", content.encode().hex(" ").upper())
        print("sha256 ", hashlib.sha256(content.encode()).hexdigest().upper())


def main() -> int:
    global V0_MODE
    common = argparse.ArgumentParser(add_help=False)
    common.add_argument("--v0", action="store_true", help="behave like a v0 (old, unencrypted) node")
    common.add_argument("--key", type=Path, help="device key file (created if missing; default: in memory)")
    common.add_argument("--trust", type=Path, help="trust store JSON file (default: in memory)")
    common.add_argument("--require-encryption", action="store_true", help="refuse plaintext sessions")
    common.add_argument("--name")
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    sub.add_parser("selftest")
    vp = sub.add_parser("vectors")
    vp.add_argument("--v1", action="store_true", help="print the v1 vectors (JSON for docs/test-vectors.json)")
    p = sub.add_parser("peers")
    p.add_argument("--wait", type=float, default=3)
    p.add_argument("--seed", action="append", default=[])
    r = sub.add_parser("receive", parents=[common])
    r.add_argument("dir", type=Path)
    r.add_argument("--port", type=int, default=TRANSFER_PORT)
    r.add_argument("--no-announce", action="store_true")
    r.add_argument("--udp-port", type=int, default=DISCOVERY_PORT, help="discovery port to bind (tests)")
    r.add_argument("--show-qr", action="store_true", help="print a QR verification payload (spec §7.7)")
    s = sub.add_parser("send", parents=[common])
    s.add_argument("target")
    s.add_argument("files", nargs="+", type=Path)
    s.add_argument("--no-direct", action="store_true", help="skip the TCP connect, use a callback (spec §6)")
    s.add_argument("--peer-udp", help="receiver discovery address HOST:PORT (default HOST:47470)")
    s.add_argument("--udp-port", type=int, default=0,
                   help="local discovery port for the callback; 47470 also catches the receiver's multicast announces")
    ab = sub.add_parser("abort-handshakes", help="play a code-guessing attacker (spec §7.3)")
    ab.add_argument("target")
    ab.add_argument("count", type=int)
    v = sub.add_parser("verify", parents=[common], help="scan side of the QR verification (spec §7.7)")
    v.add_argument("target", help="HOST:PORT of the device that shows the code")
    v.add_argument("payload")
    a = ap.parse_args()

    if a.cmd == "selftest":
        return selftest()
    if a.cmd == "vectors":
        vectors(a.v1)
        return 0
    if a.cmd == "peers":
        for m in peers(a.wait, a.seed).values():
            crypto = "v1" if isinstance(m.get("crypto"), int) and m["crypto"] >= 1 else "v0"
            print(f"{m['name']:<24} {m['platform']:<8} {crypto} {m['ip']}:{m['port']}  {m['id']}")
        return 0
    if a.cmd == "abort-handshakes":
        h, _, prt = a.target.rpartition(":")
        print(json.dumps(abort_handshakes(h, int(prt), a.count)))
        return 0
    V0_MODE = a.v0
    ident = Identity(me(a.name), a.key, a.trust, v0=a.v0, require_encryption=a.require_encryption)
    if a.cmd == "receive":
        receive(a.dir, a.port, ident, announce=not a.no_announce, udp_port=a.udp_port, show_qr=a.show_qr)
        return 0
    host, _, port = a.target.rpartition(":") if a.target.count(":") == 1 else (a.target, "", "")
    host = host or a.target
    if a.cmd == "verify":
        try:
            result = verify(host, int(port or TRANSFER_PORT), a.payload, ident)
        except (OSError, ValueError, ConnectionError) as e:
            result = {"ok": False, "error": str(e)}
        print(json.dumps(result))
        return 0 if result.get("ok") else 1
    uhost, _, uport = (a.peer_udp or host).rpartition(":") if a.peer_udp else (host, "", "")
    try:
        result = send_to(host, int(port or TRANSFER_PORT), a.files, ident, (uhost or host, int(uport or DISCOVERY_PORT)),
                         udp_port=a.udp_port, direct=not a.no_direct)
    except (OSError, ValueError, ConnectionError) as e:
        print(json.dumps({"ok": False, "error": str(e)}))
        return 1
    print(json.dumps(result, ensure_ascii=False))
    return 0 if result.get("ok") else 1


if __name__ == "__main__":
    sys.exit(main())
