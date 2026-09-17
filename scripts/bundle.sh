#!/usr/bin/env bash
# Build a macOS .app bundle for Bomb Code from the release binary.
# Output: target/release/bundle/Bomb Code.app
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

cargo build --release -p bomb_app

APP="${ROOT}/target/release/bundle/Bomb Code.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "${ROOT}/target/release/bomb_app" "$APP/Contents/MacOS/bomb_app"
cp "${ROOT}/crates/bomb_app/assets/icon.icns" "$APP/Contents/Resources/icon.icns"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Bomb Code</string>
  <key>CFBundleDisplayName</key><string>Bomb Code</string>
  <key>CFBundleIdentifier</key><string>app.bombcode.desktop</string>
  <key>CFBundleVersion</key><string>0.1.0</string>
  <key>CFBundleShortVersionString</key><string>0.1.0</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleExecutable</key><string>bomb_app</string>
  <key>CFBundleIconFile</key><string>icon</string>
  <key>LSMinimumSystemVersion</key><string>12.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
</dict>
</plist>
PLIST
echo "Bundled: $APP"
