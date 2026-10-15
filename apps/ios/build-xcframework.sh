#!/usr/bin/env bash
# Builds the Rust core (crates/hoppy-ffi, staticlib) for iOS devices and simulators and wraps
# it as apps/ios/build/HoppyFFI.xcframework (+ the C header and a module map, so Swift can
# `import HoppyFFI`). macOS + Xcode only; run from anywhere:
#
#   bash apps/ios/build-xcframework.sh [debug|release]      (default: release)
#
# Needs: rustup target add aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
profile=${1:-release}
out="$here/build"
export IPHONEOS_DEPLOYMENT_TARGET=15.0

# `release` uses the workspace's `ffi` profile (release without strip: a stripped static
# library has no exported C symbols left).
flag=()
dir=debug
if [[ $profile == release ]]; then flag=(--profile ffi); dir=ffi; fi

cd "$root"
for target in aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios; do
  echo "== cargo build $target ($profile)"
  cargo build -p hoppy-ffi --lib --target "$target" "${flag[@]}" --message-format=short
done

lib=libhoppy_ffi.a
rm -rf "$out/HoppyFFI.xcframework" "$out/headers" "$out/sim"
mkdir -p "$out/headers" "$out/sim"
cp crates/hoppy-ffi/include/hoppy.h "$out/headers/"
cat > "$out/headers/module.modulemap" <<'MAP'
module HoppyFFI {
    header "hoppy.h"
    export *
}
MAP
lipo -create "target/aarch64-apple-ios-sim/$dir/$lib" "target/x86_64-apple-ios/$dir/$lib" \
  -output "$out/sim/$lib"
xcodebuild -create-xcframework \
  -library "target/aarch64-apple-ios/$dir/$lib" -headers "$out/headers" \
  -library "$out/sim/$lib" -headers "$out/headers" \
  -output "$out/HoppyFFI.xcframework"

# Every slice must target iOS 15 or older (the iPad Air 2022 runs 18, older iPads 15+).
for f in "target/aarch64-apple-ios/$dir/$lib" "$out/sim/$lib"; do
  cmds=$(otool -l "$f" 2>/dev/null || true)
  # Highest minimum over all object files of the archive.
  ver=$(awk '/LC_BUILD_VERSION|LC_VERSION_MIN_IPHONEOS/{b=1} b&&/^ *(minos|version) /{print $2; b=0}' <<<"$cmds" |
    sort -V | tail -n1)
  echo "$f: minos ${ver:-?}"
  if [[ -n $ver ]] && [[ $(printf '%s\n15.0\n' "$ver" | sort -V | tail -n1) != 15.0 ]]; then
    echo "error: $f needs iOS $ver > 15.0" >&2
    exit 1
  fi
done
# The C entry points must be exported (global, defined) in every slice, or Xcode links with
# "undefined symbol". Output saved first: `nm | grep -q` would trip pipefail.
for f in "target/aarch64-apple-ios/$dir/$lib" "$out/sim/$lib"; do
  syms=$(nm -g "$f" 2>/dev/null || true)
  if ! grep -Eq '[[:space:]][TS][[:space:]]+_hoppy_node_start$' <<<"$syms"; then
    echo "error: _hoppy_node_start is not exported by $f" >&2
    echo "nm -g lines mentioning hoppy_:" >&2
    grep 'hoppy_' <<<"$syms" | head -n 20 >&2 || true
    all=$(nm -a "$f" 2>/dev/null || true)
    echo "nm -a lines mentioning hoppy_node_start (lowercase t = local):" >&2
    grep 'hoppy_node_start' <<<"$all" | head -n 5 >&2 || true
    exit 1
  fi
done
echo "wrote $out/HoppyFFI.xcframework"
