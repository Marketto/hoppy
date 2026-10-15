#!/usr/bin/env bash
# Builds the unsigned Hoppy-ios-unsigned.ipa (Payload/Hoppy.app with PlugIns/HoppyShare.appex)
# for sideloading with Sideloadly and a free Apple ID (docs/platforms/ios.md, ADR 0009).
# macOS + Xcode + XcodeGen (brew install xcodegen). Run from anywhere:
#
#   bash apps/ios/build-ipa.sh [VERSION] [OUT_DIR]      (default: 0.1.0, dist)
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
version=${1:-0.1.0}
outdir=${2:-dist}
[[ $outdir = /* ]] || outdir="$root/$outdir"
build=${HOPPY_BUILD_NUMBER:-1}

[[ -d "$here/build/HoppyFFI.xcframework" ]] || bash "$here/build-xcframework.sh" release
cd "$here"
# Version into the generated project only (project.yml stays as committed).
sed -e "s/MARKETING_VERSION: \".*\"/MARKETING_VERSION: \"${version%%-*}\"/" \
    -e "s/CURRENT_PROJECT_VERSION: \".*\"/CURRENT_PROJECT_VERSION: \"$build\"/" project.yml > project.release.yml
xcodegen generate --spec project.release.yml
xcodebuild -project Hoppy.xcodeproj -scheme Hoppy -configuration Release \
  -destination 'generic/platform=iOS' -derivedDataPath build/dd \
  CODE_SIGNING_ALLOWED=NO CODE_SIGNING_REQUIRED=NO CODE_SIGN_IDENTITY="" build

app=build/dd/Build/Products/Release-iphoneos/Hoppy.app
[[ -d $app/PlugIns/HoppyShare.appex ]] || { echo "error: the Share extension is missing from Hoppy.app" >&2; exit 1; }
plutil -lint "$app/Info.plist" "$app/PlugIns/HoppyShare.appex/Info.plist"
lipo -archs "$app/Hoppy"
rm -rf build/ipa && mkdir -p build/ipa/Payload "$outdir"
cp -R "$app" build/ipa/Payload/
(cd build/ipa && zip -qry "$outdir/Hoppy-ios-unsigned.ipa" Payload)
ls -l "$outdir/Hoppy-ios-unsigned.ipa"
