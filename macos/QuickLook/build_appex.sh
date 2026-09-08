#!/usr/bin/env bash
# Build the two Quick Look app extensions (no Xcode project) and, when the host app bundle exists,
# embed and re-sign them into it.
#
#   macos/QuickLook/build_appex.sh            → build/appex/StepView{Thumbnail,Preview}.appex
#                                               + embed into build/dist/StepView.app if present
#   macos/QuickLook/build_appex.sh --no-embed → build the appexes only
#
# Set STEPVIEW_NO_QL=1 to skip the whole thing (macos/build.sh honours the same variable).
set -euo pipefail
cd "$(dirname "$0")/../.."
ROOT=$PWD
. macos/signing.sh

if [ "${STEPVIEW_NO_QL:-}" = "1" ]; then
  echo "STEPVIEW_NO_QL=1 — skipping Quick Look extensions"
  exit 0
fi

EMBED=1
[ "${1:-}" = "--no-embed" ] && EMBED=0

QL=macos/QuickLook
OUT=build/appex
APP=build/dist/StepView.app
SDK=$(xcrun --sdk macosx --show-sdk-path)
SDK_VERSION=$(xcrun --sdk macosx --show-sdk-version)
DEPLOY=arm64-apple-macos13.0

# Version: whatever the host app bundle says, else the workspace version, so the appexes never
# disagree with the app they live in.
VERSION=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$APP/Contents/Info.plist" 2>/dev/null || true)
if [ -z "$VERSION" ]; then
  VERSION=$(awk '/^\[workspace.package\]/{f=1} f && /^version = /{gsub(/[",]/,"",$3); print $3; exit}' Cargo.toml)
fi
VERSION=${VERSION:-0.1.0}

echo "==> cargo build --release -p step-ffi"
cargo build --release -p step-ffi

# Frameworks the Rust staticlib pulls in: wgpu/Metal (+ QuartzCore, CoreVideo, IOKit, IOSurface)
# for the headless renderer, Security and CoreFoundation for the std side, plus libc++ and libiconv,
# which the Rust runtime references but does not carry.
COMMON_LINK=(
  -L "$ROOT/target/release" -lstep_ffi
  -framework Foundation -framework AppKit -framework CoreFoundation -framework CoreGraphics
  -framework Metal -framework MetalKit -framework QuartzCore -framework CoreVideo
  -framework IOKit -framework IOSurface -framework Security
  -lc++ -liconv
)

build_appex() {
  local name=$1 module=$2 point_fw=$3 src_dir=$4
  local bundle="$OUT/$name.appex"
  echo "==> $name.appex"
  rm -rf "$bundle"
  mkdir -p "$bundle/Contents/MacOS"
  cp "$src_dir/Info.plist" "$bundle/Contents/Info.plist"
  /usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString $VERSION" "$bundle/Contents/Info.plist"
  /usr/libexec/PlistBuddy -c "Set :CFBundleVersion $VERSION" "$bundle/Contents/Info.plist"
  /usr/libexec/PlistBuddy -c "Set :DTPlatformVersion $SDK_VERSION" "$bundle/Contents/Info.plist"
  /usr/libexec/PlistBuddy -c "Set :DTSDKName macosx$SDK_VERSION" "$bundle/Contents/Info.plist"

  # An app extension's entry point is _NSExtensionMain, not _main.
  # The Rust static library is built for the host SDK, so ld warns about every object file being
  # "newer" than the 13.0 deployment target; that noise is filtered out below, diagnostics are not.
  local log="$OUT/$name.build.log"
  local status=0
  swiftc \
    -target "$DEPLOY" -sdk "$SDK" -O -parse-as-library \
    -module-name "$module" \
    -I "$QL/include" \
    -emit-executable -o "$bundle/Contents/MacOS/$name" \
    "$src_dir"/*.swift \
    -framework "$point_fw" -framework SceneKit \
    "${COMMON_LINK[@]}" \
    -Xlinker -e -Xlinker _NSExtensionMain \
    -Xlinker -rpath -Xlinker /usr/lib/swift \
    2>"$log" || status=$?
  grep -v -e "was built for newer 'macOS' version" -e "duplicate -rpath" "$log" >&2 || true
  if [ "$status" != 0 ]; then
    echo "swiftc failed for $name (see $log)" >&2
    return "$status"
  fi

  codesign "${SIGN_FLAGS[@]}" \
    --entitlements "$src_dir/$(basename "$src_dir").entitlements" "$bundle"
  codesign --verify --verbose=1 "$bundle" 2>&1 | sed 's/^/    /'
}

mkdir -p "$OUT"
build_appex StepViewThumbnail StepViewThumbnail QuickLookThumbnailing "$QL/Thumbnail"
build_appex StepViewPreview   StepViewPreview   Quartz               "$QL/Preview"

if [ "$EMBED" = "0" ]; then
  echo "built $OUT/StepViewThumbnail.appex and $OUT/StepViewPreview.appex (not embedded)"
  exit 0
fi
if [ ! -d "$APP" ]; then
  echo "no $APP yet — build it with macos/build.sh, then re-run to embed"
  exit 0
fi

echo "==> embedding into $APP"
mkdir -p "$APP/Contents/PlugIns"
rm -rf "$APP/Contents/PlugIns/StepViewThumbnail.appex" "$APP/Contents/PlugIns/StepViewPreview.appex"
cp -R "$OUT/StepViewThumbnail.appex" "$OUT/StepViewPreview.appex" "$APP/Contents/PlugIns/"
# Sign inside-out and *without* --deep: --deep would re-sign the nested appexes with the outer
# invocation's options, silently dropping the sandbox entitlement they were just signed with, and
# pkd then refuses to register them.
codesign "${SIGN_FLAGS[@]}" "$APP"
codesign --verify --deep --verbose=1 "$APP" 2>&1 | sed 's/^/    /'
echo "embedded both extensions in $APP (version $VERSION)"
