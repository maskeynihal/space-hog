#!/bin/sh
# Builds a release binary and wraps it in "Space Hog.app" (macOS).
set -e
cd "$(dirname "$0")"
# CI passes a prebuilt (e.g. universal) binary in SPACE_HOG_BIN.
BIN="${SPACE_HOG_BIN:-target/release/space-hog}"
[ -n "$SPACE_HOG_BIN" ] || cargo build --release
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)"
APP="Space Hog.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS"
cp "$BIN" "$APP/Contents/MacOS/space-hog"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Space Hog</string>
  <key>CFBundleDisplayName</key><string>Space Hog</string>
  <key>CFBundleIdentifier</key><string>np.com.nihal.spacehog</string>
  <key>CFBundleExecutable</key><string>space-hog</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST
codesign --force --deep --sign - "$APP" 2>/dev/null || true
echo "Built $(pwd)/$APP"
