#!/bin/zsh
# Builds litty and installs it as ~/Applications/litty.app
# so you can open it from Spotlight or the Dock.
set -e
cd "$(dirname "$0")"

cargo build --release

APP="$HOME/Applications/litty.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS"
cp target/release/litty "$APP/Contents/MacOS/litty"

# App icon: icon.png (1024x1024) -> litty.icns. Replace icon.png to change the icon.
ICONSET="$(mktemp -d)/litty.iconset"
mkdir -p "$ICONSET" "$APP/Contents/Resources"
for s in 16 32 128 256 512; do
  sips -z $s $s icon.png --out "$ICONSET/icon_${s}x${s}.png" >/dev/null
  sips -z $((s * 2)) $((s * 2)) icon.png --out "$ICONSET/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/litty.icns"
rm -rf "$(dirname "$ICONSET")"

cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>litty</string>
  <key>CFBundleDisplayName</key><string>litty</string>
  <key>CFBundleIdentifier</key><string>local.litty</string>
  <key>CFBundleExecutable</key><string>litty</string>
  <key>CFBundleIconFile</key><string>litty</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleVersion</key><string>0.1.0</string>
  <key>CFBundleShortVersionString</key><string>0.1.0</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST

# Apple Silicon needs every app signed; this is a local ("ad-hoc") signature.
codesign --force --sign - "$APP"

echo "Installed: $APP"
echo "Open it with: open \"$APP\"   (or search 'litty' in Spotlight)"
