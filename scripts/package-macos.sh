#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
CLIENT_DIR="$ROOT/native-client"
DIST_ROOT="$ROOT/dist"
APP_NAME="ArtForgeStudio"
ARCH_NAME="${1:-}"

case "$ARCH_NAME" in
  x64)
    RUST_TARGET="x86_64-apple-darwin"
    ;;
  aarch64)
    RUST_TARGET="aarch64-apple-darwin"
    ;;
  *)
    echo "Usage: $0 <x64|aarch64>" >&2
    exit 2
    ;;
esac

APP_VERSION="$({
  cargo metadata --manifest-path "$ROOT/Cargo.toml" --format-version 1 --no-deps
} | python3 -c '
import json
import sys

metadata = json.load(sys.stdin)
package = next(item for item in metadata["packages"] if item["name"] == "artforge-studio-native")
print(package["version"])
')"

OUTPUT_DIR="$DIST_ROOT/$APP_NAME-macos-$ARCH_NAME"
APP_DIR="$OUTPUT_DIR/$APP_NAME.app"
CONTENTS_DIR="$APP_DIR/Contents"
MACOS_DIR="$CONTENTS_DIR/MacOS"
RESOURCES_DIR="$CONTENTS_DIR/Resources"
DMG_PATH="$DIST_ROOT/${APP_NAME}_${APP_VERSION}_macos_${ARCH_NAME}.dmg"

cargo build \
  --release \
  --locked \
  --manifest-path "$CLIENT_DIR/Cargo.toml" \
  --target "$RUST_TARGET" \
  --bin "$APP_NAME"

rm -rf "$OUTPUT_DIR"
mkdir -p "$MACOS_DIR" "$RESOURCES_DIR/assets"

cp "$ROOT/target/$RUST_TARGET/release/$APP_NAME" "$MACOS_DIR/$APP_NAME"
chmod +x "$MACOS_DIR/$APP_NAME"

if [[ -d "$CLIENT_DIR/assets" ]]; then
  cp -R "$CLIENT_DIR/assets/." "$RESOURCES_DIR/assets/"
fi
if [[ -d "$ROOT/assets" ]]; then
  cp -R "$ROOT/assets/." "$RESOURCES_DIR/assets/"
fi

mkdir -p \
  "$RESOURCES_DIR/data/input" \
  "$RESOURCES_DIR/data/out" \
  "$RESOURCES_DIR/data/prompt"

cp "$CLIENT_DIR/assets/app.icns" "$RESOURCES_DIR/app.icns"

cat > "$CONTENTS_DIR/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key>
  <string>zh_CN</string>
  <key>CFBundleExecutable</key>
  <string>$APP_NAME</string>
  <key>CFBundleIdentifier</key>
  <string>com.artforgestudio.client</string>
  <key>CFBundleName</key>
  <string>$APP_NAME</string>
  <key>CFBundleDisplayName</key>
  <string>ArtForgeStudio</string>
  <key>CFBundleIconFile</key>
  <string>app.icns</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>$APP_VERSION</string>
  <key>CFBundleVersion</key>
  <string>$APP_VERSION</string>
  <key>LSMinimumSystemVersion</key>
  <string>11.0</string>
  <key>NSHighResolutionCapable</key>
  <true/>
</dict>
</plist>
EOF

plutil -lint "$CONTENTS_DIR/Info.plist"

if [[ -n "${APPLE_SIGNING_IDENTITY:-}" ]]; then
  codesign \
    --force \
    --deep \
    --options runtime \
    --timestamp \
    --sign "$APPLE_SIGNING_IDENTITY" \
    "$APP_DIR"
  codesign --verify --deep --strict --verbose=2 "$APP_DIR"
else
  echo "APPLE_SIGNING_IDENTITY is not set; creating an unsigned development DMG."
fi

DMG_STAGE="$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/artforge-dmg.XXXXXX")"
cleanup() {
  rm -rf "$DMG_STAGE"
}
trap cleanup EXIT

cp -R "$APP_DIR" "$DMG_STAGE/$APP_NAME.app"
ln -s /Applications "$DMG_STAGE/Applications"
rm -f "$DMG_PATH"
hdiutil create \
  -volname "$APP_NAME" \
  -srcfolder "$DMG_STAGE" \
  -ov \
  -format UDZO \
  "$DMG_PATH"

echo "macOS package: $DMG_PATH"
