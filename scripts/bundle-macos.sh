#!/usr/bin/env bash
# Builds omp-desktop.app and a .dmg for macOS.
#
#   scripts/bundle-macos.sh                       # ad-hoc signed, for local use
#   scripts/bundle-macos.sh --sign "Developer ID Application: Name (TEAMID)" \
#                           --notarize my-profile # distributable
#
# Options (each also reads an env var):
#   --sign ID         codesign identity; "-" (default) is ad-hoc       SIGN_IDENTITY
#   --notarize NAME   notarytool keychain profile; needs a real --sign NOTARY_PROFILE
#                     (create once: xcrun notarytool store-credentials NAME
#                      --apple-id … --team-id … --password <app-specific>)
#   --bundle-id ID    CFBundleIdentifier (default com.omp-desktop.app)  BUNDLE_ID
#   --out DIR         output directory (default target/bundle)          OUT_DIR
#   --no-dmg          stop after the .app
#
# omp itself is not bundled: the app finds it on the login shell's PATH or in
# /opt/homebrew/bin, /usr/local/bin, ~/.local/bin.
set -euo pipefail

cd "$(dirname "$0")/.."
SIGN_IDENTITY="${SIGN_IDENTITY:--}"
NOTARY_PROFILE="${NOTARY_PROFILE:-}"
BUNDLE_ID="${BUNDLE_ID:-com.omp-desktop.app}"
OUT_DIR="${OUT_DIR:-target/bundle}"
MIN_MACOS="${MACOSX_DEPLOYMENT_TARGET:-11.0}"
DMG=1

while [[ $# -gt 0 ]]; do
	case "$1" in
		--sign) SIGN_IDENTITY="$2"; shift 2 ;;
		--notarize) NOTARY_PROFILE="$2"; shift 2 ;;
		--bundle-id) BUNDLE_ID="$2"; shift 2 ;;
		--out) OUT_DIR="$2"; shift 2 ;;
		--no-dmg) DMG=0; shift ;;
		-h|--help) sed -n '2,19p' "$0"; exit 0 ;;
		*) echo "unknown option: $1" >&2; exit 2 ;;
	esac
done

if [[ -n "$NOTARY_PROFILE" && "$SIGN_IDENTITY" == "-" ]]; then
	echo "--notarize needs a Developer ID identity (--sign); ad-hoc signatures cannot be notarized" >&2
	exit 2
fi
if [[ -n "$NOTARY_PROFILE" && $DMG -eq 0 ]]; then
	echo "--notarize submits the .dmg; drop --no-dmg" >&2
	exit 2
fi

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
ARCH="$(uname -m)"
APP="$OUT_DIR/omp-desktop.app"
ASSETS=crates/omp-desktop/assets

echo "==> cargo build --release (omp-desktop $VERSION, $ARCH, macOS ≥ $MIN_MACOS)"
MACOSX_DEPLOYMENT_TARGET="$MIN_MACOS" cargo build --release -p omp-desktop

echo "==> $APP"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/omp-desktop "$APP/Contents/MacOS/omp-desktop"
cp "$ASSETS/AppIcon.icns" "$APP/Contents/Resources/AppIcon.icns"
sed -e "s/@VERSION@/$VERSION/g" -e "s/@BUNDLE_ID@/$BUNDLE_ID/g" -e "s/@MIN_MACOS@/$MIN_MACOS/g" \
	"$ASSETS/Info.plist" > "$APP/Contents/Info.plist"
plutil -lint -s "$APP/Contents/Info.plist"
printf 'APPL????' > "$APP/Contents/PkgInfo"

# Hardened runtime is required for notarization; a secure timestamp needs a
# real identity.
sign() {
	if [[ "$SIGN_IDENTITY" == "-" ]]; then
		codesign --force --sign - --options runtime --timestamp=none "$@"
	else
		codesign --force --sign "$SIGN_IDENTITY" --options runtime --timestamp "$@"
	fi
}
echo "==> codesign ($([[ "$SIGN_IDENTITY" == "-" ]] && echo ad-hoc || echo "$SIGN_IDENTITY"))"
sign "$APP"
codesign --verify --strict --verbose=2 "$APP"

if [[ $DMG -eq 0 ]]; then
	echo "done: $APP"
	exit 0
fi

DMG_PATH="$OUT_DIR/omp-desktop-$VERSION-$ARCH.dmg"
echo "==> $DMG_PATH"
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
rm -f "$DMG_PATH"
hdiutil create -quiet -volname "omp-desktop $VERSION" -srcfolder "$STAGE" -fs HFS+ -format UDZO -ov "$DMG_PATH"
if [[ "$SIGN_IDENTITY" != "-" ]]; then
	codesign --force --sign "$SIGN_IDENTITY" --timestamp "$DMG_PATH"
fi

if [[ -n "$NOTARY_PROFILE" ]]; then
	echo "==> notarytool submit (profile $NOTARY_PROFILE)"
	xcrun notarytool submit "$DMG_PATH" --keychain-profile "$NOTARY_PROFILE" --wait
	xcrun stapler staple "$DMG_PATH"
	spctl --assess --type open --context context:primary-signature --verbose=2 "$DMG_PATH"
fi

echo "done: $APP"
echo "      $DMG_PATH"
