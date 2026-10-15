#!/usr/bin/env python3
"""Hoppy protocol v0 reference implementation (stdlib only).

Independent second implementation of docs/protocol.md, used to:
  * check conformance of the Rust core and the Android app (interop tests),
  * debug real devices from any machine with Python 3.9+.

Usage:
  hoppy_ref.py selftest                    run docs/test-vectors.json
  hoppy_ref.py peers [--wait 3]            list nearby devices
  hoppy_ref.py receive DIR [--port P]      accept everything into DIR (trusted LAN only);
                                           also answers callback requests (spec §6)
  hoppy_ref.py send HOST[:PORT] FILE...    send files; falls back to a callback if the
                                           TCP connect fails (--no-direct: callback only)
  hoppy_ref.py vectors                     print the byte-level example of spec §5
"""
from __future__ import annotations

import argparse
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
import uuid
from pathlib import Path

MAGIC = b"HOPY"
CALLBACK_MAGIC = b"HOPC"
VERSION = 0
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
def me(name: str | None = None) -> dict:
    return {"id": str(uuid.uuid4()), "name": name or f"hoppy-ref@{socket.gethostname()}"[:64], "platform": "linux"}


# ---------------------------------------------------------------- send
def send(host: str, port: int, paths: list[Path], sender: dict) -> dict:
    """Direct connection only (spec §3)."""
    with socket.create_connection((host, port), timeout=10) as s:
        return send_over(s, paths, sender)


def send_to(host: str, port: int, paths: list[Path], sender: dict, peer_udp: tuple[str, int],
            udp_port: int = 0, direct: bool = True) -> dict:
    """Direct first (3 s); if the connect fails, ask the receiver to connect back (spec §6)."""
    if direct:
        try:
            s = socket.create_connection((host, port), timeout=DIRECT_CONNECT_TIMEOUT)
        except OSError as e:
            print(f"direct connect failed ({e}); asking {host} to connect back", file=sys.stderr)
        else:
            with s:
                s.settimeout(10)
                return send_over(s, paths, sender)
    with callback_connect(peer_udp, sender, udp_port) as s:
        return send_over(s, paths, sender)


def send_over(s: socket.socket, paths: list[Path], sender: dict) -> dict:
    files = [{"name": p.name, "size": p.stat().st_size} for p in paths]
    s.sendall(MAGIC + bytes([VERSION]))
    write_json(s, {"sender": sender, "files": files})
    s.settimeout(120)
    decision = read_json(s)
    if not decision.get("accept"):
        return {"declined": decision.get("reason")}
    s.settimeout(30)
    for p in paths:
        h = hashlib.sha256()
        with p.open("rb") as f:
            while chunk := f.read(CHUNK):
                h.update(chunk)
                s.sendall(chunk)
        s.sendall(h.digest())
    return read_json(s)


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


def callback_hello(receiver_id: str, token: str) -> bytes:
    body = json.dumps({"id": receiver_id, "token": token}, separators=(",", ":"), ensure_ascii=False).encode()
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


def answer_callback(ip: str, port: int, token: str, device: dict, out: Path, limiter: CallbackLimiter) -> None:
    try:
        conn = socket.create_connection((ip, port), timeout=CALLBACK_CONNECT_TIMEOUT)
        conn.sendall(callback_hello(device["id"], token))
        limiter.mark_used(token)  # only now: a re-send after a failed connect may retry
        handle(conn, out)
    except (OSError, ValueError) as e:
        print(f"callback to {ip}:{port} failed: {e}", file=sys.stderr)
    finally:
        limiter.release(ip)


class callback_connect:
    """Sender side: context manager yielding the verified socket the receiver opened."""

    def __init__(self, peer_udp: tuple[str, int], sender: dict, udp_port: int = 0) -> None:
        self.peer_udp, self.sender, self.udp_port = peer_udp, sender, udp_port
        self.conn: socket.socket | None = None

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
        # Learn the receiver's id (needed for "to") from its answer to a unicast query.
        peer_ip = socket.gethostbyname(self.peer_udp[0])
        peer_id, target = None, (peer_ip, self.peer_udp[1])
        udp.sendto(msg("query", self.sender, 0), target)
        end = time.monotonic() + CALLBACK_WINDOW
        next_resend = 0.0
        while time.monotonic() < end:
            now = time.monotonic()
            if peer_id and now >= next_resend:
                udp.sendto(msg("callback", self.sender, port, token=token, to=peer_id), target)
                next_resend = now + CALLBACK_RESEND
            ready, _, _ = select.select([udp, srv], [], [], 0.2)
            if udp in ready:
                data, addr = udp.recvfrom(2048)
                m = decode(data)
                # Only the receiver's known IP may speak for it (spec §6 sender step 2).
                if (m and m["type"] in ("announce", "query") and m["port"] and addr[0] == peer_ip
                        and is_local_address(addr[0])):
                    if peer_id is None:
                        peer_id = m["id"]
                    if m["id"] == peer_id:  # reply inside the firewall's response window
                        target = addr
                        udp.sendto(msg("callback", self.sender, port, token=token, to=peer_id), addr)
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
                    if hmac.compare_digest(str(hello.get("token", "")).encode(), token.encode()) and hello.get("id") == peer_id:
                        conn.settimeout(10)
                        return conn
                    raise ValueError("callback token mismatch")
                except (OSError, ValueError):
                    conn.close()
        raise ConnectionError(f"{self.peer_udp[0]} did not connect directly or back within {CALLBACK_WINDOW:.0f} s")


# ---------------------------------------------------------------- receive
def handle(conn: socket.socket, out: Path, log=print) -> None:
    with conn:
        conn.settimeout(30)
        if read_exact(conn, 5) != MAGIC + bytes([VERSION]):
            return
        offer = read_json(conn)
        files = offer.get("files", [])
        names = [sanitize(f["name"]) for f in files]
        if not files or len(files) > MAX_FILES or None in names:
            write_json(conn, {"accept": False, "reason": "too_many_files" if len(files) > MAX_FILES else "invalid_offer"})
            return
        write_json(conn, {"accept": True})
        out.mkdir(parents=True, exist_ok=True)
        failed = []
        for f, name in zip(files, names):
            part = unique_path(out, name + ".hoppy-part")
            h = hashlib.sha256()
            remaining = int(f["size"])
            try:
                with part.open("xb") as w:
                    while remaining:
                        chunk = conn.recv(min(CHUNK, remaining))
                        if not chunk:
                            raise ConnectionError("closed mid-file")
                        h.update(chunk)
                        w.write(chunk)
                        remaining -= len(chunk)
                if read_exact(conn, 32) == h.digest():
                    part.rename(unique_path(out, name))
                else:
                    failed.append(name)
                    part.unlink()
            except BaseException:
                part.unlink(missing_ok=True)
                raise
        result = {"ok": True} if not failed else {"ok": False, "error": "hash mismatch: " + ", ".join(failed)}
        write_json(conn, result)
        log(f"received {len(files) - len(failed)}/{len(files)} file(s) from {offer['sender'].get('name')}")


def receive(out: Path, port: int, device: dict, announce: bool = True, udp_port: int = DISCOVERY_PORT) -> None:
    srv = socket.create_server(("0.0.0.0", port), reuse_port=hasattr(socket, "SO_REUSEPORT"))
    port = srv.getsockname()[1]
    if announce:
        threading.Thread(target=announcer, args=(device, port, out, udp_port), daemon=True).start()
    print(f"receiving as {device['name']!r} on port {port} into {out}", flush=True)
    while True:
        conn, _ = srv.accept()
        threading.Thread(target=handle, args=(conn, out), daemon=True).start()


# ---------------------------------------------------------------- discovery
def msg(kind: str, device: dict, port: int, **extra: str) -> bytes:
    return json.dumps({"hoppy": VERSION, "type": kind, **device, "port": port, **extra}).encode()


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


def announcer(device: dict, port: int, out: Path | None = None, udp_port: int = DISCOVERY_PORT) -> None:
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
                             args=(addr[0], m["port"], m["token"], device, out, limiter)).start()


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
        if json.loads(msg("callback", dev, m["port"], token=m["token"], to=m["to"])) != json.loads(cb["datagram_json"]):
            bad += 1
            print("FAIL callback datagram encoding")
    for raw in cb["invalid_datagrams"]:
        if decode(raw.encode()) is not None:
            bad += 1
            print(f"FAIL accepted invalid callback datagram {raw}")
    h = json.loads(cb["hello_json"])
    if callback_hello(h["id"], h["token"]).hex() != cb["hello_hex"]:
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
    print("selftest:", "OK" if not bad else f"{bad} failure(s)")
    return 1 if bad else 0


def callback_listener_checks() -> bool:
    """Sender side of spec §6 step 3 on loopback: oversized, non-object and slow hellos are
    closed cleanly; a connection from another IP (127.0.0.2) is closed unread."""
    sender = me("selftest-tx")
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
        ok &= isinstance(result.get("conn"), socket.socket)
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


def vectors() -> None:
    ex = json.loads((ROOT / "docs" / "test-vectors.json").read_text())["offer_example"]
    body = ex["offer_json"].encode()
    print("hello  ", (MAGIC + bytes([VERSION])).hex(" ").upper())
    print("length ", struct.pack(">I", len(body)).hex(" ").upper(), f"({len(body)} bytes of JSON)")
    for name, content in ex["files_content_utf8"].items():
        print("content", content.encode().hex(" ").upper())
        print("sha256 ", hashlib.sha256(content.encode()).hexdigest().upper())


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    sub.add_parser("selftest")
    sub.add_parser("vectors")
    p = sub.add_parser("peers")
    p.add_argument("--wait", type=float, default=3)
    p.add_argument("--seed", action="append", default=[])
    r = sub.add_parser("receive")
    r.add_argument("dir", type=Path)
    r.add_argument("--port", type=int, default=TRANSFER_PORT)
    r.add_argument("--name")
    r.add_argument("--no-announce", action="store_true")
    r.add_argument("--udp-port", type=int, default=DISCOVERY_PORT, help="discovery port to bind (tests)")
    s = sub.add_parser("send")
    s.add_argument("target")
    s.add_argument("files", nargs="+", type=Path)
    s.add_argument("--no-direct", action="store_true", help="skip the TCP connect, use a callback (spec §6)")
    s.add_argument("--peer-udp", help="receiver discovery address HOST:PORT (default HOST:47470)")
    s.add_argument("--udp-port", type=int, default=0,
                   help="local discovery port for the callback; 47470 also catches the receiver's multicast announces")
    a = ap.parse_args()

    if a.cmd == "selftest":
        return selftest()
    if a.cmd == "vectors":
        vectors()
        return 0
    if a.cmd == "peers":
        for m in peers(a.wait, a.seed).values():
            print(f"{m['name']:<24} {m['platform']:<8} {m['ip']}:{m['port']}  {m['id']}")
        return 0
    if a.cmd == "receive":
        receive(a.dir, a.port, me(a.name), announce=not a.no_announce, udp_port=a.udp_port)
        return 0
    host, _, port = a.target.rpartition(":") if a.target.count(":") == 1 else (a.target, "", "")
    host = host or a.target
    uhost, _, uport = (a.peer_udp or host).rpartition(":") if a.peer_udp else (host, "", "")
    try:
        result = send_to(host, int(port or TRANSFER_PORT), a.files, me(), (uhost or host, int(uport or DISCOVERY_PORT)),
                         udp_port=a.udp_port, direct=not a.no_direct)
    except (OSError, ConnectionError) as e:
        print(json.dumps({"ok": False, "error": str(e)}))
        return 1
    print(json.dumps(result))
    return 0 if result.get("ok") else 1


if __name__ == "__main__":
    sys.exit(main())
