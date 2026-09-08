#!/usr/bin/env bash
# Build, bundle, sign and (optionally) install StepView.app.
#   macos/build.sh            → build/dist/StepView.app
#   macos/build.sh --install  → also copy to /Applications and register with Launch Services
# Signing identity: ad-hoc by default, or STEPVIEW_SIGN_IDENTITY="Developer ID Application: …"
# (see macos/signing.sh). For a notarized .dmg/.zip use macos/package.sh, which calls this script.
set -euo pipefail
cd "$(dirname "$0")/.."
. macos/signing.sh

cargo build --release
if [ ! -f macos/AppIcon.icns ]; then
  macos/make_icon.sh
fi
cargo packager --release --formats app --out-dir build/dist

APP=build/dist/StepView.app
# cargo-packager ad-hoc signs the bundle; re-sign with the selected identity (nothing is nested yet,
# so --deep is harmless here).
codesign --deep "${SIGN_FLAGS[@]}" "$APP"

# Quick Look thumbnail + preview extensions: build, drop into $APP/Contents/PlugIns, re-sign the app.
# It runs *after* the --deep signature above on purpose — --deep would re-sign the nested appexes
# without their entitlements. STEPVIEW_NO_QL=1 skips the extensions entirely.
if [ "${STEPVIEW_NO_QL:-}" != "1" ]; then
  macos/QuickLook/build_appex.sh
fi
LSREG=/System/Library/Frameworks/CoreServices.framework/Versions/A/Frameworks/LaunchServices.framework/Versions/A/Support/lsregister
# Register the build copy with Launch Services only when nothing is installed. Launch Services prefers
# the /Applications copy, but a registered duplicate with the same bundle id and version still shows up
# in "Open With" lists and muddies default-handler debugging. (Spotlight may re-register it on its own.)
if [ ! -d /Applications/StepView.app ]; then
  "$LSREG" -f "$APP" >/dev/null 2>&1 || true
fi
echo "built $APP"

if [ "${1:-}" = "--install" ]; then
  rm -rf /Applications/StepView.app
  cp -R "$APP" /Applications/StepView.app
  # Drop the build copy's registration so only the installed one is listed (see above).
  "$LSREG" -u "$APP" >/dev/null 2>&1 || true
  "$LSREG" -f /Applications/StepView.app >/dev/null 2>&1 || true
  echo "installed /Applications/StepView.app (double-click a .step/.stp file or use Open With)"
fi
