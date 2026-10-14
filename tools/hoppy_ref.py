#!/usr/bin/env python3
"""Hoppy protocol v0 reference implementation (stdlib only).

Independent second implementation of docs/protocol.md, used to:
  * check conformance of the Rust core and the Android app (interop tests),
  * debug real devices from any machine with Python 3.9+.

Usage:
  hoppy_ref.py selftest                    run docs/test-vectors.json
  hoppy_ref.py peers [--wait 3]            list nearby devices
  hoppy_ref.py receive DIR [--port P]      accept everything into DIR (trusted LAN only)
  hoppy_ref.py send HOST[:PORT] FILE...    send files
  hoppy_ref.py vectors                     print the byte-level example of spec §5
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import socket
import struct
import sys
import threading
import time
import uuid
from pathlib import Path

MAGIC = b"HOPY"
VERSION = 0
DISCOVERY_PORT = 47470
TRANSFER_PORT = 47471
MCAST = "239.255.72.80"
MAX_FRAME = 1 << 20
MAX_FILES = 10_000
CHUNK = 256 * 1024
ROOT = Path(__file__).resolve().parent.parent
RESERVED = {"CON", "PRN", "AUX", "NUL", *(f"COM{i}" for i in range(1, 10)), *(f"LPT{i}" for i in range(1, 10))}


# ---------------------------------------------------------------- framing
def read_exact(sock: socket.socket, n: int) -> bytes:
    buf = bytearray()
    while len(buf) < n:
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


def read_json(sock: socket.socket) -> dict:
    (n,) = struct.unpack(">I", read_exact(sock, 4))
    if n > MAX_FRAME:
        raise ValueError(f"frame of {n} bytes exceeds limit")
    return json.loads(read_exact(sock, n))


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
    files = [{"name": p.name, "size": p.stat().st_size} for p in paths]
    with socket.create_connection((host, port), timeout=10) as s:
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


def receive(out: Path, port: int, device: dict, announce: bool = True) -> None:
    srv = socket.create_server(("0.0.0.0", port), reuse_port=hasattr(socket, "SO_REUSEPORT"))
    port = srv.getsockname()[1]
    if announce:
        threading.Thread(target=announcer, args=(device, port), daemon=True).start()
    print(f"receiving as {device['name']!r} on port {port} into {out}", flush=True)
    while True:
        conn, _ = srv.accept()
        threading.Thread(target=handle, args=(conn, out), daemon=True).start()


# ---------------------------------------------------------------- discovery
def msg(kind: str, device: dict, port: int) -> bytes:
    return json.dumps({"hoppy": VERSION, "type": kind, **device, "port": port}).encode()


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


def announcer(device: dict, port: int) -> None:
    s = udp_socket(DISCOVERY_PORT)
    s.settimeout(0.5)
    broadcast(s, msg("query", device, port))
    next_announce = 0.0
    while True:
        if time.monotonic() >= next_announce:
            broadcast(s, msg("announce", device, port))
            next_announce = time.monotonic() + 5
        try:
            data, addr = s.recvfrom(2048)
            m = json.loads(data)
            if m.get("hoppy") == VERSION and m.get("type") == "query" and m.get("id") != device["id"]:
                s.sendto(msg("announce", device, port), addr)
        except (socket.timeout, ValueError):
            pass


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
        try:
            m = json.loads(data)
        except ValueError:
            continue
        if m.get("hoppy") == VERSION and m.get("type") in ("announce", "query") and m.get("port"):
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
    print("selftest:", "OK" if not bad else f"{bad} failure(s)")
    return 1 if bad else 0


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
    s = sub.add_parser("send")
    s.add_argument("target")
    s.add_argument("files", nargs="+", type=Path)
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
        receive(a.dir, a.port, me(a.name), announce=not a.no_announce)
        return 0
    host, _, port = a.target.rpartition(":") if a.target.count(":") == 1 else (a.target, "", "")
    result = send(host or a.target, int(port or TRANSFER_PORT), a.files, me())
    print(json.dumps(result))
    return 0 if result.get("ok") else 1


if __name__ == "__main__":
    sys.exit(main())
