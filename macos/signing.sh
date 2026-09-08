# Sourced by macos/build.sh, macos/QuickLook/build_appex.sh and macos/package.sh.
#
# STEPVIEW_SIGN_IDENTITY selects the codesign identity for everything in the bundle:
#   unset or "-"                               ad-hoc (local development; the default)
#   "Developer ID Application: NAME (TEAMID)"  distribution: also turns on the hardened runtime and a
#                                              secure timestamp, both required for notarization
# Every codesign call in the build goes through SIGN_FLAGS so the app, the two Quick Look appexes
# and (in package.sh) the DMG all carry the same identity.
SIGN_IDENTITY=${STEPVIEW_SIGN_IDENTITY:--}
SIGN_FLAGS=(--force --sign "$SIGN_IDENTITY")
if [ "$SIGN_IDENTITY" != "-" ]; then
  SIGN_FLAGS+=(--options runtime --timestamp)
fi
