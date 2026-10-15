#!/usr/bin/env bash
# Hoppy for macOS: Hoppy.app (universal x86_64 + arm64, macOS 12+) and its .dmg.
# Runs on a Mac (CI: macos-latest); see docs/platforms/macos-ios.md and docs/release.md.
#
#   bash apps/macos/build-app.sh release [VERSION] [OUT_DIR]
#       builds both architectures (MACOSX_DEPLOYMENT_TARGET=12.0), lipo, Hoppy.app, checks
#       (architectures, minimum macOS, signature, Info.plist), then
#       OUT_DIR/Hoppy-macos-universal.dmg (Hoppy.app + Applications link + first-launch help).
#       VERSION defaults to the workspace version in Cargo.toml; OUT_DIR to dist.
#   bash apps/macos/build-app.sh bundle HOPPYW HOPPY APP_DIR [VERSION]
#       only assembles and signs APP_DIR (e.g. target/smoke/Hoppy.app) from given binaries.
#
# Signing: HOPPY_SIGN_IDENTITY (e.g. "Developer ID Application: … (TEAMID)", from a keychain
# set up by release.yml) signs with the hardened runtime + timestamp, ready for
# notarization. Without it the app is signed ad hoc (`codesign -s -`): it runs on Apple
# silicon, and Gatekeeper asks for "Open Anyway" on first launch.
# CI pipes the output through tools/ci-annotate.sh on failure (errors become annotations).
set -euo pipefail
cd "$(dirname "$0")/../.."

export MACOSX_DEPLOYMENT_TARGET=${MACOSX_DEPLOYMENT_TARGET:-12.0}
MIN_MACOS=12.0
TARGETS=(x86_64-apple-darwin aarch64-apple-darwin)
IDENTITY=${HOPPY_SIGN_IDENTITY:-}

fail() {
  echo "::error title=macOS package::$*"
  exit 1
}

workspace_version() {
  sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1
}

sign() { # path [extra codesign args…]
  local path=$1
  shift
  if [[ -n $IDENTITY ]]; then
    codesign --force --options runtime --timestamp --sign "$IDENTITY" "$@" "$path"
  else
    codesign --force --sign - "$@" "$path"
  fi
}

# bundle HOPPYW HOPPY APP_DIR VERSION
bundle() {
  local gui=$1 cli=$2 app=$3 version=$4
  # Semver only (it goes into sed and Info.plist); macOS wants plain numbers in the bundle
  # versions, so a pre-release suffix ("-rc1") is dropped there.
  [[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$ ]] ||
    fail "version '$version' is not x.y.z or x.y.z-suffix"
  local numeric=${version%%-*}
  [[ -x $gui && -x $cli ]] || fail "missing binaries: $gui $cli"
  rm -rf "$app"
  mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
  cp "$gui" "$app/Contents/MacOS/hoppyw"
  # The CLI sits next to the app's executable (lower-case "hoppy" never clashes with
  # "Hoppy" because the app executable is "hoppyw").
  cp "$cli" "$app/Contents/MacOS/hoppy"
  sed "s/@VERSION@/$numeric/g" apps/macos/Info.plist >"$app/Contents/Info.plist"
  printf 'APPL????' >"$app/Contents/PkgInfo"
  cp assets/brand/macos/Hoppy.icns "$app/Contents/Resources/Hoppy.icns"
  cp -R apps/macos/en.lproj apps/macos/it.lproj "$app/Contents/Resources/"
  cp LICENSE "$app/Contents/Resources/LICENSE.txt"
  plutil -lint "$app/Contents/Info.plist" "$app"/Contents/Resources/*.lproj/*.strings
  # Nested code first, then the bundle (seals Info.plist and resources).
  sign "$app/Contents/MacOS/hoppy"
  sign "$app"
  codesign --verify --strict --verbose=2 "$app"
}

# "yes" if version A <= version B (numeric, up to three parts).
version_le() {
  awk -v a="$1" -v b="$2" 'BEGIN {
    split(a, x, "."); split(b, y, ".")
    for (i = 1; i <= 3; i++) {
      if (x[i] + 0 < y[i] + 0) { print "yes"; exit }
      if (x[i] + 0 > y[i] + 0) { print "no"; exit }
    }
    print "yes"
  }'
}

# Every architecture of BINARY must run on macOS $MIN_MACOS (LC_BUILD_VERSION minos).
check_min_macos() {
  local bin=$1 arch minos
  for arch in x86_64 arm64; do
    minos=$(otool -arch "$arch" -l "$bin" | awk '/LC_BUILD_VERSION/{b=1} b&&$1=="minos"{print $2; exit}')
    [[ -n $minos ]] || fail "$bin ($arch): no LC_BUILD_VERSION"
    if [[ $(version_le "$minos" "$MIN_MACOS") != yes ]]; then
      fail "$bin ($arch) needs macOS $minos, must run on $MIN_MACOS (MACOSX_DEPLOYMENT_TARGET)"
    fi
    echo "$(basename "$bin") $arch: minos $minos"
  done
}

release() {
  local version=${1:-$(workspace_version)} out=${2:-dist}
  local t log
  for t in "${TARGETS[@]}"; do
    log="build-$t.log"
    # Errors stay in the output: the workflow step turns it into annotations.
    cargo build --release -p hoppy --target "$t" --message-format=short 2>&1 | tee "$log"
  done
  local work=target/macos
  rm -rf "$work"
  mkdir -p "$work/bin" "$out"
  local b
  for b in hoppy hoppyw; do
    lipo -create -output "$work/bin/$b" \
      "target/x86_64-apple-darwin/release/$b" "target/aarch64-apple-darwin/release/$b"
    lipo "$work/bin/$b" -verify_arch x86_64 arm64 || fail "$b is not universal"
    check_min_macos "$work/bin/$b"
  done
  local app="$work/dmg/Hoppy.app"
  mkdir -p "$work/dmg"
  bundle "$work/bin/hoppyw" "$work/bin/hoppy" "$app" "$version"
  "$app/Contents/MacOS/hoppy" --version
  ln -s /Applications "$work/dmg/Applications"
  cp "apps/macos/Primo avvio - First launch.txt" "$work/dmg/"
  local dmg="$out/Hoppy-macos-universal.dmg"
  rm -f "$dmg"
  # hdiutil is known to fail now and then on CI runners ("Resource busy"): retry.
  local try
  for try in 1 2 3; do
    hdiutil create -volname Hoppy -srcfolder "$work/dmg" -fs HFS+ -format UDZO -ov "$dmg" && break
    [[ $try == 3 ]] && fail "hdiutil create failed 3 times"
    sleep 5
  done
  if [[ -n $IDENTITY ]]; then
    codesign --force --timestamp --sign "$IDENTITY" "$dmg"
  fi
  hdiutil verify "$dmg"
  ls -l "$dmg"
}

case ${1:-} in
  release) shift; release "$@" ;;
  bundle)
    shift
    [[ $# -ge 3 ]] || fail "usage: build-app.sh bundle HOPPYW HOPPY APP_DIR [VERSION]"
    bundle "$1" "$2" "$3" "${4:-$(workspace_version)}"
    ;;
  *) fail "usage: build-app.sh release [VERSION] [OUT_DIR] | bundle HOPPYW HOPPY APP_DIR [VERSION]" ;;
esac
