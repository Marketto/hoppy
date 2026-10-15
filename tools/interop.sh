#!/usr/bin/env bash
# Cross-implementation test: Rust CLI <-> Python reference, both directions, plus discovery,
# the callback fallback (spec §6) and protocol v1 (spec §7: encryption, same SAS on both
# ends, QR verification, fallback to/from v0 nodes). Run from the repo root: tools/interop.sh
#
# The Python reference encrypts in pure Python (~1 MB/s): its encrypted legs use a 256 KB
# file. Until the Android core implements v1 (phase 2), the Kotlin legs run against v0 and
# check that Rust and Python fall back to plaintext with it.
set -euo pipefail
export LC_ALL=C.UTF-8  # JVM needs a UTF-8 locale for non-ASCII file names

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
REF="python3 $ROOT/tools/hoppy_ref.py"
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

fail() { echo "   FAIL: $*" >&2; exit 1; }
# The 8-digit SAS ("1234 5678") in a line of output.
digits() { grep -oE '[0-9]{4} [0-9]{4}' | head -1; }
# Waits until FILE has a line matching PATTERN (background receivers print asynchronously).
wait_for() {
  for _ in $(seq 1 50); do grep -q "$2" "$1" 2>/dev/null && return 0; sleep 0.1; done
  fail "no '$2' in $1: $(cat "$1" 2>/dev/null)"
}

echo "1/12 Rust → Python (encrypted, same code on both screens)"
$REF receive "$WORK/py-in" --port 47481 --no-announce >"$WORK/py-in.log" 2>&1 &
PIDS+=($!); sleep 0.7
"$BIN" send --to 127.0.0.1:47481 "$WORK/small.bin" "$WORK/nota 🐰.txt" >"$WORK/rs-send.log"
cmp "$WORK/small.bin" "$WORK/py-in/small.bin"
cmp "$WORK/nota 🐰.txt" "$WORK/py-in/nota 🐰.txt"
grep -q "🔒 encrypted" "$WORK/rs-send.log" || fail "Rust sender did not encrypt"
wait_for "$WORK/py-in.log" "offer from"
RS_CODE="$(digits < "$WORK/rs-send.log")"
PY_CODE="$(grep "offer from" "$WORK/py-in.log" | digits)"
[[ -n "$RS_CODE" && "$RS_CODE" == "$PY_CODE" ]] || fail "codes differ: Rust '$RS_CODE', Python '$PY_CODE'"
echo "   code $RS_CODE on both"

echo "2/12 Python → Rust (encrypted)"
"$BIN" receive --yes --port 47482 --dir "$WORK/rs-in" >"$WORK/rs-in.log" &
PIDS+=($!); sleep 0.7
$REF send 127.0.0.1:47482 "$WORK/small.bin" >"$WORK/py-send.json" 2>"$WORK/py-send.log"
sleep 0.3
cmp "$WORK/small.bin" "$WORK/rs-in/small.bin"
grep -q '"encrypted": true' "$WORK/py-send.json" || fail "Python sender did not encrypt: $(cat "$WORK/py-send.json")"
PY_CODE="$(digits < "$WORK/py-send.log")"
wait_for "$WORK/rs-in.log" "🔒 encrypted"
RS_CODE="$(grep "🔒 encrypted" "$WORK/rs-in.log" | tail -1 | digits)"
[[ -n "$RS_CODE" && "$RS_CODE" == "$PY_CODE" ]] || fail "codes differ: Python '$PY_CODE', Rust '$RS_CODE'"

echo "3/12 discovery: Python sees the Rust receiver (v1)"
if $REF peers --wait 2 --seed 127.0.0.1:47470 | grep InteropRust | grep -q " v1 "; then
  echo "   found"
else
  fail "Rust receiver not discovered as v1"
fi

# Callback (spec §6): the sender skips or fails the direct TCP connect and the receiver
# connects back after a `callback` datagram on its discovery port.
echo "4/12 callback: Rust sender → Python receiver connects back (encrypted)"
$REF receive "$WORK/py-cb" --port 47581 --udp-port 47590 --name InteropPy >/dev/null 2>&1 &
PIDS+=($!)
"$BIN" receive --yes --port 47584 --udp-port 47591 --dir "$WORK/rs-cb" >/dev/null &
PIDS+=($!); sleep 0.7
"$BIN" send --seed 127.0.0.1:47590 --to InteropPy --no-direct \
  "$WORK/small.bin" "$WORK/nota 🐰.txt" >"$WORK/rs-cb-send.log"
cmp "$WORK/small.bin" "$WORK/py-cb/small.bin"
cmp "$WORK/nota 🐰.txt" "$WORK/py-cb/nota 🐰.txt"
grep -q "🔒 encrypted" "$WORK/rs-cb-send.log" || fail "callback session not encrypted"

echo "5/12 callback: Python sender (direct refused) → Rust receiver connects back (encrypted)"
$REF send 127.0.0.1:1 "$WORK/small.bin" --peer-udp 127.0.0.1:47591 >"$WORK/py-cb.json" 2>/dev/null
sleep 0.3
cmp "$WORK/small.bin" "$WORK/rs-cb/small.bin"
grep -q '"encrypted": true' "$WORK/py-cb.json" || fail "callback session not encrypted"

echo "6/12 v1 → v0: Rust sender, old (v0) Python receiver: plaintext, flagged"
$REF receive "$WORK/py-v0" --port 47485 --no-announce --v0 >/dev/null 2>&1 &
PIDS+=($!); sleep 0.7
"$BIN" send --to 127.0.0.1:47485 "$WORK/nota 🐰.txt" >"$WORK/rs-v0.log"
cmp "$WORK/nota 🐰.txt" "$WORK/py-v0/nota 🐰.txt"
grep -q "NOT encrypted" "$WORK/rs-v0.log" || fail "plaintext not flagged"

echo "7/12 v0 → v1: old (v0) Python sender → Rust receiver: accepted, flagged"
$REF send 127.0.0.1:47482 "$WORK/nota 🐰.txt" --v0 >/dev/null 2>&1
sleep 0.3
cmp "$WORK/nota 🐰.txt" "$WORK/rs-in/nota 🐰.txt"
wait_for "$WORK/rs-in.log" "NOT encrypted"

echo "8/12 callback + v0: Rust sender → old (v0) Python receiver connects back: plaintext"
$REF receive "$WORK/py-v0cb" --port 47585 --udp-port 47593 --name InteropPyOld --v0 >/dev/null 2>&1 &
PIDS+=($!); sleep 0.7
"$BIN" send --seed 127.0.0.1:47593 --to InteropPyOld --no-direct "$WORK/nota 🐰.txt" >"$WORK/rs-v0cb.log"
cmp "$WORK/nota 🐰.txt" "$WORK/py-v0cb/nota 🐰.txt"
grep -q "NOT encrypted" "$WORK/rs-v0cb.log" || fail "plaintext not flagged"

echo "9/12 v1 Python sender → old (v0) Python receiver (reference falls back too)"
$REF send 127.0.0.1:47485 "$WORK/small.bin" >"$WORK/py-v0.json" 2>/dev/null
cmp "$WORK/small.bin" "$WORK/py-v0/small.bin"
grep -q '"encrypted": false' "$WORK/py-v0.json" || fail "fallback not flagged"

echo "10/12 QR: Python scans the Rust receiver's code → Rust marks Python verified"
"$BIN" receive --yes --port 47486 --udp-port 47594 --show-qr --dir "$WORK/rs-qr" >"$WORK/rs-qr.log" &
PIDS+=($!)
wait_for "$WORK/rs-qr.log" "QR verification code"
PAYLOAD="$(grep -oE 'hoppy:verify:1:[^ ]+' "$WORK/rs-qr.log")"
$REF verify 127.0.0.1:47486 "$PAYLOAD" --name InteropScanner >"$WORK/py-verify.json" 2>&1 \
  || fail "verify refused: $(cat "$WORK/py-verify.json")"
"$BIN" trust list | grep "✓" | grep -q InteropScanner || fail "Rust did not mark the scanner verified"
if $REF verify 127.0.0.1:47486 "$PAYLOAD" >/dev/null 2>&1; then fail "QR code accepted twice"; fi

echo "11/12 QR: Rust scans the Python receiver's code → both verified"
$REF receive "$WORK/py-qr" --port 47487 --no-announce --show-qr --name InteropPyQr >"$WORK/py-qr.log" 2>&1 &
PIDS+=($!)
wait_for "$WORK/py-qr.log" "^qr "
"$BIN" send --to 127.0.0.1:47487 "$WORK/nota 🐰.txt" >/dev/null  # first contact: Rust remembers its key
PAYLOAD="$(grep -oE 'hoppy:verify:1:[^ ]+' "$WORK/py-qr.log")"
"$BIN" trust verify --qr "$PAYLOAD" --to 127.0.0.1:47487 >"$WORK/rs-verify.log" \
  || fail "Rust could not verify: $(cat "$WORK/rs-verify.log")"
grep -q "now shows this device as verified" "$WORK/rs-verify.log" || fail "not confirmed"
wait_for "$WORK/py-qr.log" "verified InteropRust"

echo "12/12 aborted handshakes (code guessing) are limited on both receivers"
OUT="$($REF abort-handshakes 127.0.0.1:47486 7)"
[[ "$OUT" == '{"served": 5, "refused": 2}' ]] || fail "Rust receiver: $OUT"
wait_for "$WORK/rs-qr.log" "broken off right after the handshake"
OUT="$($REF abort-handshakes 127.0.0.1:47487 7)"
[[ "$OUT" == '{"served": 5, "refused": 2}' ]] || fail "Python receiver: $OUT"
if "$BIN" send --to 127.0.0.1:47487 "$WORK/nota 🐰.txt" >"$WORK/rs-busy.log" 2>&1; then
  fail "a refused receiver must not get a plaintext fallback"
fi
grep -q "busy" "$WORK/rs-busy.log" || fail "no busy: $(cat "$WORK/rs-busy.log")"

if [[ -n "${HOPPY_KT_CP:-}" ]]; then
  # Classpath of the compiled Android :core module + its tests (CI sets this, see tools/dev.py).
  # The Kotlin core is still v0: every leg below is plaintext through the v0 fallback.
  KT="java -cp $HOPPY_KT_CP app.hoppy.core.InteropMainKt"
  echo "K1/6 Kotlin(Android core, v0) → Rust"
  $KT send 127.0.0.1 47482 "$WORK/nota 🐰.txt" >/dev/null
  sleep 0.3
  cmp "$WORK/nota 🐰.txt" "$WORK/rs-in/nota 🐰 (1).txt"
  echo "K2/6 Rust → Kotlin (v1 hello refused, plaintext fallback)"
  $KT receive "$WORK/kt-in" 47483 47592 >/dev/null &
  PIDS+=($!); sleep 1.5
  "$BIN" send --to 127.0.0.1:47483 "$WORK/blob.bin" >"$WORK/rs-kt.log"
  sleep 0.3
  cmp "$WORK/blob.bin" "$WORK/kt-in/blob.bin"
  grep -q "NOT encrypted" "$WORK/rs-kt.log" || fail "plaintext not flagged"
  echo "K3/6 discovery: Kotlin sees the Rust receiver"
  $KT peers 47470 | grep -q InteropRust || fail "Rust receiver not discovered by Kotlin"
  echo "K4/6 callback: Kotlin sender → Rust receiver connects back"
  $KT send-peer 47591 --no-direct "$WORK/nota 🐰.txt" >/dev/null
  sleep 0.3
  cmp "$WORK/nota 🐰.txt" "$WORK/rs-cb/nota 🐰.txt"
  echo "K5/6 callback: Rust sender → Kotlin receiver connects back"
  "$BIN" send --seed 127.0.0.1:47592 --to InteropKotlin --no-direct "$WORK/nota 🐰.txt" >/dev/null
  sleep 0.3
  cmp "$WORK/nota 🐰.txt" "$WORK/kt-in/nota 🐰.txt"
  echo "K6/6 callback: Python sender (direct refused) → Kotlin receiver connects back"
  $REF send 127.0.0.1:1 "$WORK/blob.bin" --peer-udp 127.0.0.1:47592 >/dev/null 2>&1
  sleep 0.3
  cmp "$WORK/blob.bin" "$WORK/kt-in/blob (1).bin"
fi
echo "interop OK"
