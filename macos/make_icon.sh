#!/usr/bin/env bash
# Render the app icon with stepview itself (a small assembly from the test fixtures) and build the .icns.
set -euo pipefail
cd "$(dirname "$0")/.."
BIN=target/release/stepview
[ -x "$BIN" ] || cargo build --release
SRC=tests/fixtures/cube_mm.step
TMP=build/icon
rm -rf "$TMP" && mkdir -p "$TMP/AppIcon.iconset"
"$BIN" render "$SRC" -o "$TMP/icon_1024.png" --size 1024x1024 --view iso --bg '#2b2f36' --no-cache >/dev/null
for s in 16 32 128 256 512; do
  sips -z $s $s "$TMP/icon_1024.png" --out "$TMP/AppIcon.iconset/icon_${s}x${s}.png" >/dev/null
  d=$((s*2))
  sips -z $d $d "$TMP/icon_1024.png" --out "$TMP/AppIcon.iconset/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$TMP/AppIcon.iconset" -o macos/AppIcon.icns
echo "wrote macos/AppIcon.icns"
