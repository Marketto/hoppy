#!/usr/bin/env bash
# Cross-implementation test: Rust CLI <-> Python reference, both directions, plus discovery,
# the callback fallback (spec §6) and protocol v1 (spec §7: encryption, same SAS on both
# ends, fallback to/from v0 nodes). Run from the repo root: tools/interop.sh
#
# The Python reference encrypts in pure Python (~1 MB/s): its encrypted legs use a 256 KB
# file. The Kotlin legs (K1–K9, with HOPPY_KT_CP) run the Android core against both, encrypted,
# plus its fallback to a v0 (old) app and the aborted-handshake limit.
set -euo pipefail
export LC_ALL=C.UTF-8  # JVM needs a UTF-8 locale for non-ASCII file names

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
export PYTHONUNBUFFERED=1  # receivers' logs are read from files while they run
REF="python3 -u $ROOT/tools/hoppy_ref.py"
cargo build -q --manifest-path "$ROOT/Cargo.toml" -p hoppy
BIN="$ROOT/target/debug/hoppy"
WORK="$(mktemp -d)"
PIDS=()
cleanup() { for p in "${PIDS[@]}"; do kill "$p" 2>/dev/null || true; done; rm -rf "$WORK"; }
trap cleanup EXIT

head -c 3000000 /dev/urandom > "$WORK/blob.bin"
head -c 262144 /dev/urandom > "$WORK/small.bin"
printf 'ciao' > "$WORK/nota 🐰.txt"
export HOPPY_CONFIG_DIR="$WORK/cfg"
"$BIN" name "InteropRust" >/dev/null

# On failure, the tail of every log of this run (CI shows the last lines as an annotation).
fail() {
  echo "   FAIL: $*" >&2
  for f in "$WORK"/*.log "$WORK"/*.json; do
    [[ -f $f ]] || continue
    echo "--- $(basename "$f") (last 15 lines)" >&2
    tail -n 15 "$f" >&2
  done
  exit 1
}
# The SAS (4 emoji, spec §7.6) in a line of output: the first run of exactly 4 symbol code
# points standing alone (the 🔒 marker and "🐰" in file names are single code points).
sas() { python3 -c 'import re, sys; m = re.search(r"(?<!\S)[\u2300-\U0001FFFF]{4}(?!\S)", sys.stdin.read()); print(m.group(0) if m else "")'; }
# Waits until FILE has a line matching PATTERN (background receivers print asynchronously).
wait_for() {
  for _ in $(seq 1 50); do grep -q "$2" "$1" 2>/dev/null && return 0; sleep 0.1; done
  fail "no '$2' in $1: $(cat "$1" 2>/dev/null)"
}

echo "1/10 Rust → Python (encrypted, same code on both screens)"
$REF receive "$WORK/py-in" --port 47481 --no-announce >"$WORK/py-in.log" 2>&1 &
PIDS+=($!); sleep 0.7
"$BIN" send --to 127.0.0.1:47481 "$WORK/small.bin" "$WORK/nota 🐰.txt" >"$WORK/rs-send.log"
cmp "$WORK/small.bin" "$WORK/py-in/small.bin"
cmp "$WORK/nota 🐰.txt" "$WORK/py-in/nota 🐰.txt"
grep -q "🔒 encrypted" "$WORK/rs-send.log" || fail "Rust sender did not encrypt"
wait_for "$WORK/py-in.log" "offer from"
RS_CODE="$(sas < "$WORK/rs-send.log")"
PY_CODE="$(grep "offer from" "$WORK/py-in.log" | sas)"
[[ -n "$RS_CODE" && "$RS_CODE" == "$PY_CODE" ]] || fail "codes differ: Rust '$RS_CODE', Python '$PY_CODE'"
echo "   code $RS_CODE on both"

echo "2/10 Python → Rust (encrypted)"
"$BIN" receive --yes --port 47482 --dir "$WORK/rs-in" >"$WORK/rs-in.log" &
PIDS+=($!); sleep 0.7
$REF send 127.0.0.1:47482 "$WORK/small.bin" >"$WORK/py-send.json" 2>"$WORK/py-send.log"
sleep 0.3
cmp "$WORK/small.bin" "$WORK/rs-in/small.bin"
grep -q '"encrypted": true' "$WORK/py-send.json" || fail "Python sender did not encrypt: $(cat "$WORK/py-send.json")"
PY_CODE="$(sas < "$WORK/py-send.log")"
wait_for "$WORK/rs-in.log" "🔒 encrypted"
RS_CODE="$(grep "🔒 encrypted" "$WORK/rs-in.log" | tail -1 | sas)"
[[ -n "$RS_CODE" && "$RS_CODE" == "$PY_CODE" ]] || fail "codes differ: Python '$PY_CODE', Rust '$RS_CODE'"

echo "3/10 discovery: Python sees the Rust receiver (v1)"
if $REF peers --wait 2 --seed 127.0.0.1:47470 | grep InteropRust | grep -q " v1 "; then
  echo "   found"
else
  fail "Rust receiver not discovered as v1"
fi

# Callback (spec §6): the sender skips or fails the direct TCP connect and the receiver
# connects back after a `callback` datagram on its discovery port.
echo "4/10 callback: Rust sender → Python receiver connects back (encrypted)"
$REF receive "$WORK/py-cb" --port 47581 --udp-port 47590 --name InteropPy >/dev/null 2>&1 &
PIDS+=($!)
"$BIN" receive --yes --port 47584 --udp-port 47591 --dir "$WORK/rs-cb" >/dev/null &
PIDS+=($!); sleep 0.7
"$BIN" send --seed 127.0.0.1:47590 --to InteropPy --no-direct \
  "$WORK/small.bin" "$WORK/nota 🐰.txt" >"$WORK/rs-cb-send.log"
cmp "$WORK/small.bin" "$WORK/py-cb/small.bin"
cmp "$WORK/nota 🐰.txt" "$WORK/py-cb/nota 🐰.txt"
grep -q "🔒 encrypted" "$WORK/rs-cb-send.log" || fail "callback session not encrypted"

echo "5/10 callback: Python sender (direct refused) → Rust receiver connects back (encrypted)"
$REF send 127.0.0.1:1 "$WORK/small.bin" --peer-udp 127.0.0.1:47591 >"$WORK/py-cb.json" 2>/dev/null
sleep 0.3
cmp "$WORK/small.bin" "$WORK/rs-cb/small.bin"
grep -q '"encrypted": true' "$WORK/py-cb.json" || fail "callback session not encrypted"

echo "6/10 v1 → v0: Rust sender, old (v0) Python receiver: plaintext, flagged"
$REF receive "$WORK/py-v0" --port 47485 --no-announce --v0 >/dev/null 2>&1 &
PIDS+=($!); sleep 0.7
"$BIN" send --to 127.0.0.1:47485 "$WORK/nota 🐰.txt" >"$WORK/rs-v0.log"
cmp "$WORK/nota 🐰.txt" "$WORK/py-v0/nota 🐰.txt"
grep -q "NOT encrypted" "$WORK/rs-v0.log" || fail "plaintext not flagged"

echo "7/10 v0 → v1: old (v0) Python sender → Rust receiver: accepted, flagged"
$REF send 127.0.0.1:47482 "$WORK/nota 🐰.txt" --v0 >/dev/null 2>&1
sleep 0.3
cmp "$WORK/nota 🐰.txt" "$WORK/rs-in/nota 🐰.txt"
wait_for "$WORK/rs-in.log" "NOT encrypted"

echo "8/10 callback + v0: Rust sender → old (v0) Python receiver connects back: plaintext"
$REF receive "$WORK/py-v0cb" --port 47585 --udp-port 47593 --name InteropPyOld --v0 >/dev/null 2>&1 &
PIDS+=($!); sleep 0.7
"$BIN" send --seed 127.0.0.1:47593 --to InteropPyOld --no-direct "$WORK/nota 🐰.txt" >"$WORK/rs-v0cb.log"
cmp "$WORK/nota 🐰.txt" "$WORK/py-v0cb/nota 🐰.txt"
grep -q "NOT encrypted" "$WORK/rs-v0cb.log" || fail "plaintext not flagged"

echo "9/10 v1 Python sender → old (v0) Python receiver (reference falls back too)"
$REF send 127.0.0.1:47485 "$WORK/small.bin" >"$WORK/py-v0.json" 2>/dev/null
cmp "$WORK/small.bin" "$WORK/py-v0/small.bin"
grep -q '"encrypted": false' "$WORK/py-v0.json" || fail "fallback not flagged"

echo "10/10 aborted handshakes (code guessing) are limited on both receivers"
"$BIN" receive --yes --port 47486 --udp-port 47594 --dir "$WORK/rs-ab" >"$WORK/rs-ab.log" &
PIDS+=($!)
$REF receive "$WORK/py-ab" --port 47487 --no-announce --name InteropPyAb >"$WORK/py-ab.log" 2>&1 &
PIDS+=($!); sleep 0.7
OUT="$($REF abort-handshakes 127.0.0.1:47486 7)"
[[ "$OUT" == '{"served": 5, "refused": 2}' ]] || fail "Rust receiver: $OUT"
wait_for "$WORK/rs-ab.log" "broken off right after the handshake"
OUT="$($REF abort-handshakes 127.0.0.1:47487 7)"
[[ "$OUT" == '{"served": 5, "refused": 2}' ]] || fail "Python receiver: $OUT"
if "$BIN" send --to 127.0.0.1:47487 "$WORK/nota 🐰.txt" >"$WORK/rs-busy.log" 2>&1; then
  fail "a refused receiver must not get a plaintext fallback"
fi
grep -q "busy" "$WORK/rs-busy.log" || fail "no busy: $(cat "$WORK/rs-busy.log")"

if [[ -n "${HOPPY_KT_CP:-}" ]]; then
  # Classpath of the compiled Android :core module + its tests (CI sets this, see tools/dev.py).
  # The Kotlin core speaks v1 (phase 2 of ADR 0010): every leg is encrypted unless noted.
  KT="java -cp $HOPPY_KT_CP app.hoppy.core.InteropMainKt"
  # Key + trust store shared by every Kotlin process, like the app's filesDir.
  export HOPPY_KT_DIR="$WORK/kt-cfg"
  mkdir -p "$HOPPY_KT_DIR"
  # Waits until FILE has more than N lines matching PATTERN.
  wait_more() {
    for _ in $(seq 1 50); do (( $(grep -c "$2" "$1" 2>/dev/null || true) > $3 )) && return 0; sleep 0.1; done
    fail "no new '$2' in $1: $(cat "$1" 2>/dev/null)"
  }

  echo "K1/9 Kotlin(Android core) → Rust (encrypted, same code on both screens)"
  N="$(grep -c "🔒 encrypted" "$WORK/rs-in.log" || true)"
  $KT send 127.0.0.1 47482 "$WORK/nota 🐰.txt" >"$WORK/kt-send.log" || fail "$(cat "$WORK/kt-send.log")"
  cmp "$WORK/nota 🐰.txt" "$WORK/rs-in/nota 🐰 (1).txt"
  grep -q "session 🔒 encrypted" "$WORK/kt-send.log" || fail "Kotlin sender did not encrypt: $(cat "$WORK/kt-send.log")"
  wait_more "$WORK/rs-in.log" "🔒 encrypted" "$N"
  KT_CODE="$(sas < "$WORK/kt-send.log")"
  RS_CODE="$(grep "🔒 encrypted" "$WORK/rs-in.log" | tail -1 | sas)"
  [[ -n "$KT_CODE" && "$KT_CODE" == "$RS_CODE" ]] || fail "codes differ: Kotlin '$KT_CODE', Rust '$RS_CODE'"
  echo "   code $KT_CODE on both"

  echo "K2/9 Rust → Kotlin (encrypted, same code)"
  $KT receive "$WORK/kt-in" 47483 47592 >"$WORK/kt-in.log" 2>&1 &
  PIDS+=($!)
  wait_for "$WORK/kt-in.log" "receiving on"
  "$BIN" send --to 127.0.0.1:47483 "$WORK/blob.bin" >"$WORK/rs-kt.log"
  sleep 0.3
  cmp "$WORK/blob.bin" "$WORK/kt-in/blob.bin"
  grep -q "🔒 encrypted" "$WORK/rs-kt.log" || fail "Rust → Kotlin not encrypted: $(cat "$WORK/rs-kt.log")"
  RS_CODE="$(sas < "$WORK/rs-kt.log")"
  KT_CODE="$(grep "offer from InteropRust" "$WORK/kt-in.log" | tail -1 | sas)"
  [[ -n "$RS_CODE" && "$RS_CODE" == "$KT_CODE" ]] || fail "codes differ: Rust '$RS_CODE', Kotlin '$KT_CODE'"

  echo "K3/9 discovery: Kotlin sees the Rust receiver as v1"
  $KT peers 47470 | grep InteropRust | grep -q " v1 " || fail "Rust receiver not discovered as v1 by Kotlin"

  echo "K4/9 callback: Kotlin sender → Rust receiver connects back (encrypted)"
  $KT send-peer 47591 --no-direct "$WORK/nota 🐰.txt" >"$WORK/kt-cb.log" || fail "$(cat "$WORK/kt-cb.log")"
  sleep 0.3
  cmp "$WORK/nota 🐰.txt" "$WORK/rs-cb/nota 🐰.txt"
  grep -q "session 🔒 encrypted" "$WORK/kt-cb.log" || fail "callback session not encrypted"

  echo "K5/9 callback: Rust sender → Kotlin receiver connects back (encrypted)"
  "$BIN" send --seed 127.0.0.1:47592 --to InteropKotlin --no-direct "$WORK/nota 🐰.txt" >"$WORK/rs-ktcb.log"
  sleep 0.3
  cmp "$WORK/nota 🐰.txt" "$WORK/kt-in/nota 🐰.txt"
  grep -q "🔒 encrypted" "$WORK/rs-ktcb.log" || fail "callback session not encrypted"

  echo "K6/9 callback: Python sender (direct refused) → Kotlin receiver connects back (encrypted)"
  $REF send 127.0.0.1:1 "$WORK/small.bin" --peer-udp 127.0.0.1:47592 >"$WORK/py-ktcb.json" 2>/dev/null
  sleep 0.3
  cmp "$WORK/small.bin" "$WORK/kt-in/small.bin"
  grep -q '"encrypted": true' "$WORK/py-ktcb.json" || fail "callback session not encrypted"

  echo "K7/9 Python ↔ Kotlin direct (encrypted, same code)"
  $REF send 127.0.0.1:47483 "$WORK/nota 🐰.txt" >"$WORK/py-kt.json" 2>"$WORK/py-kt.log"
  sleep 0.3
  cmp "$WORK/nota 🐰.txt" "$WORK/kt-in/nota 🐰 (1).txt"
  PY_CODE="$(sas < "$WORK/py-kt.log")"
  KT_CODE="$(grep "offer from" "$WORK/kt-in.log" | tail -1 | sas)"
  [[ -n "$PY_CODE" && "$PY_CODE" == "$KT_CODE" ]] || fail "codes differ: Python '$PY_CODE', Kotlin '$KT_CODE'"
  $REF receive "$WORK/py-kt-in" --port 47489 --no-announce >"$WORK/py-kt-in.log" 2>&1 &
  PIDS+=($!); sleep 0.7
  $KT send 127.0.0.1 47489 "$WORK/small.bin" >"$WORK/kt-py.log" || fail "$(cat "$WORK/kt-py.log")"
  cmp "$WORK/small.bin" "$WORK/py-kt-in/small.bin"
  wait_for "$WORK/py-kt-in.log" "offer from"
  KT_CODE="$(sas < "$WORK/kt-py.log")"
  PY_CODE="$(grep "offer from" "$WORK/py-kt-in.log" | sas)"
  [[ -n "$KT_CODE" && "$KT_CODE" == "$PY_CODE" ]] || fail "codes differ: Kotlin '$KT_CODE', Python '$PY_CODE'"

  echo "K8/9 v1 ↔ v0: Kotlin with an old (v0) Python app: plaintext both ways, flagged"
  $KT send 127.0.0.1 47485 "$WORK/small.bin" >"$WORK/kt-v0.log" || fail "$(cat "$WORK/kt-v0.log")"
  cmp "$WORK/small.bin" "$WORK/py-v0/small (1).bin"
  grep -q "session NOT encrypted" "$WORK/kt-v0.log" || fail "plaintext not flagged"
  $REF send 127.0.0.1:47483 "$WORK/small.bin" --v0 >/dev/null 2>&1
  sleep 0.3
  cmp "$WORK/small.bin" "$WORK/kt-in/small (1).bin"
  grep "offer from" "$WORK/kt-in.log" | tail -1 | grep -q "NOT encrypted" || fail "v0 offer not flagged"

  echo "K9/9 aborted handshakes are limited on the Kotlin receiver; busy never falls back"
  OUT="$($REF abort-handshakes 127.0.0.1:47483 7)"
  [[ "$OUT" == '{"served": 5, "refused": 2}' ]] || fail "Kotlin receiver: $OUT"
  wait_for "$WORK/kt-in.log" "broken off right after the handshake"
  if $KT send 127.0.0.1 47483 "$WORK/nota 🐰.txt" >"$WORK/kt-busy.log" 2>&1; then
    fail "a refused receiver must not get a plaintext fallback"
  fi
  grep -q "ReceiverBusyException" "$WORK/kt-busy.log" || fail "no busy: $(cat "$WORK/kt-busy.log")"
fi
echo "interop OK"
