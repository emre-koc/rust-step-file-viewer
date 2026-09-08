#!/usr/bin/env bash
# Build, bundle, ad-hoc sign and (optionally) install StepView.app.
#   macos/build.sh            → build/dist/StepView.app
#   macos/build.sh --install  → also copy to /Applications and register with Launch Services
set -euo pipefail
cd "$(dirname "$0")/.."

cargo build --release
if [ ! -f macos/AppIcon.icns ]; then
  macos/make_icon.sh
fi
cargo packager --release --formats app --out-dir build/dist

APP=build/dist/StepView.app
# cargo-packager signs with the configured identity ("-" = ad-hoc); re-sign to be safe after any edits.
codesign --force --deep --sign - "$APP"

# Quick Look thumbnail + preview extensions: build, drop into $APP/Contents/PlugIns, re-sign the app.
# It runs *after* the --deep signature above on purpose — --deep would re-sign the nested appexes
# without their entitlements. STEPVIEW_NO_QL=1 skips the extensions entirely.
if [ "${STEPVIEW_NO_QL:-}" != "1" ]; then
  macos/QuickLook/build_appex.sh
fi
LSREG=/System/Library/Frameworks/CoreServices.framework/Versions/A/Frameworks/LaunchServices.framework/Versions/A/Support/lsregister
"$LSREG" -f "$APP" >/dev/null 2>&1 || true
echo "built $APP"

if [ "${1:-}" = "--install" ]; then
  rm -rf /Applications/StepView.app
  cp -R "$APP" /Applications/StepView.app
  "$LSREG" -f /Applications/StepView.app >/dev/null 2>&1 || true
  echo "installed /Applications/StepView.app (double-click a .step/.stp file or use Open With)"
fi
