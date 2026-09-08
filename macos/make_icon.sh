#!/usr/bin/env bash
# Build macos/AppIcon.icns. Uses macos/icon_source.png (1024x1024, designed icon) when present,
# otherwise renders a fallback icon with stepview itself from a small test fixture.
set -euo pipefail
cd "$(dirname "$0")/.."
TMP=build/icon
rm -rf "$TMP" && mkdir -p "$TMP/AppIcon.iconset"
if [ -f macos/icon_source.png ]; then
  cp macos/icon_source.png "$TMP/icon_1024.png"
else
  BIN=target/release/stepview
  [ -x "$BIN" ] || cargo build --release
  "$BIN" render tests/fixtures/cube_mm.step -o "$TMP/icon_1024.png" --size 1024x1024 --view iso --bg '#2b2f36' --no-cache >/dev/null
fi
for s in 16 32 128 256 512; do
  sips -z $s $s "$TMP/icon_1024.png" --out "$TMP/AppIcon.iconset/icon_${s}x${s}.png" >/dev/null
  d=$((s*2))
  sips -z $d $d "$TMP/icon_1024.png" --out "$TMP/AppIcon.iconset/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$TMP/AppIcon.iconset" -o macos/AppIcon.icns
echo "wrote macos/AppIcon.icns from $([ -f macos/icon_source.png ] && echo macos/icon_source.png || echo 'rendered fallback')"
