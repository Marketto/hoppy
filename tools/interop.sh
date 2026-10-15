#!/usr/bin/env bash
# Cross-implementation test: Rust CLI <-> Python reference, both directions,
# plus discovery and the callback fallback (spec §6). Run from the repo root: tools/interop.sh
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
printf 'ciao' > "$WORK/nota 🐰.txt"
export HOPPY_CONFIG_DIR="$WORK/cfg"
"$BIN" name "InteropRust" >/dev/null

echo "1/5 Rust → Python"
$REF receive "$WORK/py-in" --port 47481 --no-announce >/dev/null &
PIDS+=($!); sleep 0.7
"$BIN" send --to 127.0.0.1:47481 "$WORK/blob.bin" "$WORK/nota 🐰.txt" >/dev/null
cmp "$WORK/blob.bin" "$WORK/py-in/blob.bin"
cmp "$WORK/nota 🐰.txt" "$WORK/py-in/nota 🐰.txt"

echo "2/5 Python → Rust"
"$BIN" receive --yes --port 47482 --dir "$WORK/rs-in" >/dev/null &
PIDS+=($!); sleep 0.7
$REF send 127.0.0.1:47482 "$WORK/blob.bin" >/dev/null
sleep 0.3
cmp "$WORK/blob.bin" "$WORK/rs-in/blob.bin"

echo "3/5 discovery: Python sees the Rust receiver"
if $REF peers --wait 2 --seed 127.0.0.1:47470 | grep -q InteropRust; then
  echo "   found"
else
  echo "   FAIL: Rust receiver not discovered" >&2
  exit 1
fi

# Callback (spec §6): the sender skips or fails the direct TCP connect and the receiver
# connects back after a `callback` datagram on its discovery port.
echo "4/5 callback: Rust sender → Python receiver connects back"
$REF receive "$WORK/py-cb" --port 47581 --udp-port 47590 --name InteropPy >/dev/null &
PIDS+=($!)
"$BIN" receive --yes --port 47584 --udp-port 47591 --dir "$WORK/rs-cb" >/dev/null &
PIDS+=($!); sleep 0.7
"$BIN" send --seed 127.0.0.1:47590 --to InteropPy --no-direct \
  "$WORK/blob.bin" "$WORK/nota 🐰.txt" >/dev/null
cmp "$WORK/blob.bin" "$WORK/py-cb/blob.bin"
cmp "$WORK/nota 🐰.txt" "$WORK/py-cb/nota 🐰.txt"

echo "5/5 callback: Python sender (direct refused) → Rust receiver connects back"
$REF send 127.0.0.1:1 "$WORK/blob.bin" --peer-udp 127.0.0.1:47591 >/dev/null 2>&1
sleep 0.3
cmp "$WORK/blob.bin" "$WORK/rs-cb/blob.bin"
if [[ -n "${HOPPY_KT_CP:-}" ]]; then
  # Classpath of the compiled Android :core module + its tests (CI sets this, see tools/dev.py).
  KT="java -cp $HOPPY_KT_CP app.hoppy.core.InteropMainKt"
  echo "K1/6 Kotlin(Android core) → Rust"
  $KT send 127.0.0.1 47482 "$WORK/nota 🐰.txt" >/dev/null
  sleep 0.3
  cmp "$WORK/nota 🐰.txt" "$WORK/rs-in/nota 🐰.txt"
  echo "K2/6 Rust → Kotlin"
  $KT receive "$WORK/kt-in" 47483 47592 >/dev/null &
  PIDS+=($!); sleep 1.5
  "$BIN" send --to 127.0.0.1:47483 "$WORK/blob.bin" >/dev/null
  sleep 0.3
  cmp "$WORK/blob.bin" "$WORK/kt-in/blob.bin"
  echo "K3/6 discovery: Kotlin sees the Rust receiver"
  $KT peers 47470 | grep -q InteropRust || { echo "   FAIL" >&2; exit 1; }
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
