#!/usr/bin/env bash
# Cross-implementation test: Rust CLI <-> Python reference, both directions,
# plus discovery. Run from the repo root: tools/interop.sh
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

echo "1/3 Rust → Python"
$REF receive "$WORK/py-in" --port 47481 --no-announce >/dev/null &
PIDS+=($!); sleep 0.7
"$BIN" send --to 127.0.0.1:47481 "$WORK/blob.bin" "$WORK/nota 🐰.txt" >/dev/null
cmp "$WORK/blob.bin" "$WORK/py-in/blob.bin"
cmp "$WORK/nota 🐰.txt" "$WORK/py-in/nota 🐰.txt"

echo "2/3 Python → Rust"
"$BIN" receive --yes --port 47482 --dir "$WORK/rs-in" >/dev/null &
PIDS+=($!); sleep 0.7
$REF send 127.0.0.1:47482 "$WORK/blob.bin" >/dev/null
sleep 0.3
cmp "$WORK/blob.bin" "$WORK/rs-in/blob.bin"

echo "3/3 discovery: Python sees the Rust receiver"
if $REF peers --wait 2 --seed 127.0.0.1:47470 | grep -q InteropRust; then
  echo "   found"
else
  echo "   FAIL: Rust receiver not discovered" >&2
  exit 1
fi
if [[ -n "${HOPPY_KT_CP:-}" ]]; then
  # Classpath of the compiled Android :core module + its tests (CI sets this, see tools/dev.py).
  KT="java -cp $HOPPY_KT_CP app.hoppy.core.InteropMainKt"
  echo "4/6 Kotlin(Android core) → Rust"
  $KT send 127.0.0.1 47482 "$WORK/nota 🐰.txt" >/dev/null
  sleep 0.3
  cmp "$WORK/nota 🐰.txt" "$WORK/rs-in/nota 🐰.txt"
  echo "5/6 Rust → Kotlin"
  $KT receive "$WORK/kt-in" 47483 >/dev/null &
  PIDS+=($!); sleep 1.5
  "$BIN" send --to 127.0.0.1:47483 "$WORK/blob.bin" >/dev/null
  sleep 0.3
  cmp "$WORK/blob.bin" "$WORK/kt-in/blob.bin"
  echo "6/6 discovery: Kotlin sees the Rust receiver"
  $KT peers 47470 | grep -q InteropRust || { echo "   FAIL" >&2; exit 1; }
fi
echo "interop OK"
