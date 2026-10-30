#!/bin/sh
# Builds a release binary and wraps it in "Space Hog.app" (macOS).
set -e
cd "$(dirname "$0")"
cargo build --release
APP="Space Hog.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS"
cp target/release/space-hog "$APP/Contents/MacOS/space-hog"
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
  <key>CFBundleVersion</key><string>0.1.0</string>
  <key>CFBundleShortVersionString</key><string>0.1.0</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST
codesign --force --deep --sign - "$APP" 2>/dev/null || true
echo "Built $(pwd)/$APP"
