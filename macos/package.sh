#!/usr/bin/env bash
# Build, sign, notarize (when credentials are present) and package StepView for distribution.
#   macos/package.sh → build/dist/StepView-<version>-macos-arm64.{dmg,zip} + SHA256SUMS.txt
# Releases are built and published from a Mac (see README "Release signing"); CI never signs.
#
# Environment:
#   STEPVIEW_SIGN_IDENTITY  "Developer ID Application: NAME (TEAMID)"; unset/"-" = ad-hoc (macos/signing.sh)
#   Notarization credentials for notarytool, one of:
#     STEPVIEW_NOTARY_PROFILE   profile saved with `xcrun notarytool store-credentials <name>` (recommended)
#     APPLE_ID + APPLE_APP_PASSWORD (app-specific password) + APPLE_TEAM_ID
#     APP_STORE_CONNECT_API_KEY (path to the .p8) + APP_STORE_CONNECT_KEY_ID + APP_STORE_CONNECT_ISSUER_ID
#   Without them the output is signed but not notarized, and Gatekeeper still blocks it on macOS 14+.
#   STEPVIEW_SKIP_BUILD=1   package the existing build/dist/StepView.app instead of running macos/build.sh
set -euo pipefail
cd "$(dirname "$0")/.."
. macos/signing.sh

VERSION=$(grep -m1 '^version = ' Cargo.toml | sed 's/version = "\(.*\)"/\1/')
DIST=build/dist
APP=$DIST/StepView.app
BASE=StepView-${VERSION}-macos-arm64
DMG=$DIST/$BASE.dmg
ZIP=$DIST/$BASE.zip

if [ "${STEPVIEW_SKIP_BUILD:-}" != "1" ]; then
  macos/build.sh
fi
[ -d "$APP" ] || { echo "missing $APP" >&2; exit 1; }

# --- notarization credentials ---------------------------------------------------------------
NOTARIZE=0
notary_args=()
if [ "$SIGN_IDENTITY" = "-" ]; then
  echo "==> ad-hoc identity: not notarizing (set STEPVIEW_SIGN_IDENTITY to a Developer ID identity)"
elif [ -n "${STEPVIEW_NOTARY_PROFILE:-}" ]; then
  NOTARIZE=1
  notary_args=(--keychain-profile "$STEPVIEW_NOTARY_PROFILE")
elif [ -n "${APP_STORE_CONNECT_API_KEY:-}" ]; then
  NOTARIZE=1
  notary_args=(--key "$APP_STORE_CONNECT_API_KEY" --key-id "$APP_STORE_CONNECT_KEY_ID" --issuer "$APP_STORE_CONNECT_ISSUER_ID")
elif [ -n "${APPLE_APP_PASSWORD:-}" ]; then
  NOTARIZE=1
  notary_args=(--apple-id "$APPLE_ID" --password "$APPLE_APP_PASSWORD" --team-id "$APPLE_TEAM_ID")
else
  echo "==> no notarization credentials: output will be signed but NOT notarized"
fi

# Submit one file and wait for Apple's verdict. notarytool can exit 0 on a rejected submission, so
# check the status line and print the notarization log on anything but Accepted.
notarize() {
  local out id
  echo "==> notarizing $1"
  out=$(xcrun notarytool submit "$1" "${notary_args[@]}" --wait --timeout 30m 2>&1) || { printf '%s\n' "$out"; return 1; }
  printf '%s\n' "$out"
  if ! grep -q 'status: Accepted' <<<"$out"; then
    id=$(sed -n 's/^ *id: //p' <<<"$out" | head -1)
    [ -z "$id" ] || xcrun notarytool log "$id" "${notary_args[@]}" || true
    return 1
  fi
}

if [ "$NOTARIZE" = 1 ]; then
  # Notarize the bundle first and staple the ticket to it, so the copies inside both the zip and the
  # DMG pass Gatekeeper even offline. The DMG is then notarized and stapled as its own item.
  tmp=$(mktemp -d)
  ditto -c -k --keepParent "$APP" "$tmp/StepView.zip"
  notarize "$tmp/StepView.zip"
  rm -rf "$tmp"
  xcrun stapler staple "$APP"
fi

echo "==> $DMG"
rm -f "$DMG" "$ZIP"
hdiutil create -volname StepView -srcfolder "$APP" -ov -format UDZO -quiet "$DMG"
if [ "$SIGN_IDENTITY" != "-" ]; then
  codesign --force --sign "$SIGN_IDENTITY" --timestamp "$DMG"
fi
if [ "$NOTARIZE" = 1 ]; then
  notarize "$DMG"
  xcrun stapler staple "$DMG"
fi

echo "==> $ZIP"
ditto -c -k --keepParent "$APP" "$ZIP"
(cd "$DIST" && shasum -a 256 "$BASE.dmg" "$BASE.zip" | tee SHA256SUMS.txt)

echo "==> verifying"
codesign --verify --deep --strict --verbose=1 "$APP"
if [ "$SIGN_IDENTITY" != "-" ]; then
  codesign --verify --verbose=1 "$DMG"
fi
if [ "$NOTARIZE" = 1 ]; then
  spctl -a -t exec -vv "$APP"                                   # accepted, source=Notarized Developer ID
  spctl -a -t open --context context:primary-signature -v "$DMG"
else
  spctl -a -t exec -vv "$APP" 2>&1 | sed 's/^/    /' || true   # informational: rejected until notarized
fi
