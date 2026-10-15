#!/usr/bin/env python3
"""Turns a failed xcodebuild / build-script log into GitHub annotations (agents can read
annotations, not CI logs). The xcodebuild twin of tools/ci-annotate.sh, used by the iOS jobs
in .github/workflows/ci.yml and release.yml:

    python3 tools/ci_annotate_xcode.py LABEL LOGFILE

GitHub shows at most 10 error annotations per step, so this emits, in priority order:
  - compiler/XCTest diagnostics "path:line[:col]: error: …" (deduplicated), anchored at the
    file relative to the repository; failing XCTest cases;
  - linker blocks ("Undefined symbols…", "ld: …", "clang: error: …") with a few lines of context;
  - "The following build commands failed:" block, plain "error:" lines, cargo "error[…]";
  - 1 annotation with the last 40 log lines (passing test lines left out).
"""
from __future__ import annotations

import os
import re
import sys

MAX = 9  # + the tail = 10
DIAG = re.compile(r"^(?P<file>/[^:\n]+?):(?P<line>\d+)(?::(?P<col>\d+))?: (?:fatal )?error: (?P<msg>.*)$")
TEST_FAIL = re.compile(r"^Test Case '-\[(?P<name>[^\]]+)\]' failed")
CARGO = re.compile(r"^(?P<file>[^ :]+\.rs):(?P<line>\d+):(?P<col>\d+): error(?:\[\w+\])?: (?P<msg>.*)$")
BLOCK_START = re.compile(r"^(Undefined symbols for architecture|ld: |clang: error:|The following build commands failed:|error: )")


def esc(s: str) -> str:
    return s.replace("%", "%25").replace("\r", "").replace("\n", "%0A")


def prop(s: str) -> str:
    return esc(s).replace(":", "%3A").replace(",", "%2C")


def rel(path: str) -> str:
    root = os.environ.get("GITHUB_WORKSPACE") or os.getcwd()
    root = root.rstrip("/") + "/"
    return path[len(root):] if path.startswith(root) else path


def main() -> int:
    label, log = sys.argv[1], sys.argv[2]
    try:
        lines = open(log, encoding="utf-8", errors="replace").read().splitlines()
    except OSError as e:
        print(f"::error title={prop(label)}::cannot read {esc(log)}: {esc(str(e))}")
        return 0
    out: list[str] = []
    seen: set[str] = set()

    def emit(title: str, body: str, file: str | None = None, line: str | None = None, col: str | None = None) -> None:
        key = f"{file}:{line}:{body}"
        if key in seen or len(out) >= MAX:
            return
        seen.add(key)
        where = ""
        if file:
            where = f"file={prop(rel(file))},line={line or 1}" + (f",col={col}" if col else "") + ","
        out.append(f"::error {where}title={prop(title)}::{esc(body)}")

    for raw in lines:
        m = DIAG.match(raw) or CARGO.match(raw)
        if m:
            emit(label, m["msg"], m["file"], m["line"], m["col"])
    for raw in lines:
        m = TEST_FAIL.match(raw)
        if m:
            emit(f"{label}: {m['name']}", raw)
    i = 0
    while i < len(lines):
        if BLOCK_START.match(lines[i]) and not DIAG.match(lines[i]):
            block = [lines[i]]
            j = i + 1
            while j < len(lines) and j < i + 12 and (lines[j].startswith((" ", "\t")) or lines[j].startswith("ld: ")):
                block.append(lines[j])
                j += 1
            emit(label, "\n".join(block))
            i = j
        else:
            i += 1
    tail = [l for l in lines if not re.match(r"^Test Case .* passed", l) and not re.match(r"^test .* \.\.\. ok$", l)][-40:]
    out.append(f"::error title={prop(label + ' (last 40 log lines)')}::{esc(chr(10).join(tail))}")
    print("\n".join(out))
    return 0


if __name__ == "__main__":
    sys.exit(main())
