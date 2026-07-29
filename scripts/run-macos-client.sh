#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
CLIENT_DIR="$ROOT/native-client"
APP_NAME="ElunviCanvas"
TARGET_DIR="$ROOT/target/debug"
APP_DIR="$TARGET_DIR/$APP_NAME.app"
CONTENTS_DIR="$APP_DIR/Contents"
MACOS_DIR="$CONTENTS_DIR/MacOS"
RESOURCES_DIR="$CONTENTS_DIR/Resources"

APP_VERSION="$({
  cargo metadata --manifest-path "$ROOT/Cargo.toml" --format-version 1 --no-deps
} | python3 -c '
import json
import sys

metadata = json.load(sys.stdin)
package = next(item for item in metadata["packages"] if item["name"] == "artforge-studio-native")
print(package["version"])
')"

cargo build \
  --manifest-path "$ROOT/Cargo.toml" \
  -p artforge-studio-native \
  --bin "$APP_NAME"

if osascript -e 'tell application id "com.artforgestudio.client.debug" to quit' >/dev/null 2>&1; then
  sleep 1
fi

rm -rf "$APP_DIR"
mkdir -p "$MACOS_DIR" "$RESOURCES_DIR/assets"

cp "$TARGET_DIR/$APP_NAME" "$MACOS_DIR/$APP_NAME"
chmod +x "$MACOS_DIR/$APP_NAME"

if [[ -d "$CLIENT_DIR/assets" ]]; then
  cp -R "$CLIENT_DIR/assets/." "$RESOURCES_DIR/assets/"
fi
if [[ -d "$ROOT/assets" ]]; then
  cp -R "$ROOT/assets/." "$RESOURCES_DIR/assets/"
fi
if [[ -f "$CLIENT_DIR/assets/app.icns" ]]; then
  cp "$CLIENT_DIR/assets/app.icns" "$RESOURCES_DIR/app.icns"
fi

mkdir -p \
  "$RESOURCES_DIR/data/input" \
  "$RESOURCES_DIR/data/out" \
  "$RESOURCES_DIR/data/prompt"

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
  <string>com.artforgestudio.client.debug</string>
  <key>CFBundleName</key>
  <string>$APP_NAME</string>
  <key>CFBundleDisplayName</key>
  <string>Elunvi Canvas Dev</string>
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

plutil -lint "$CONTENTS_DIR/Info.plist" >/dev/null
open "$APP_DIR"
for attempt in {1..20}; do
  if osascript \
    -e 'tell application "System Events"' \
    -e 'tell application process "ElunviCanvas"' \
    -e 'if not (exists window 1) then error "window is not ready"' \
    -e 'set position of window 1 to {80, 80}' \
    -e 'set frontmost to true' \
    -e 'perform action "AXRaise" of window 1' \
    -e 'end tell' \
    -e 'end tell' >/dev/null 2>&1; then
    break
  fi
  sleep 0.1
done
echo "Started $APP_DIR"
