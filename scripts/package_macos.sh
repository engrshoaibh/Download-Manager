#!/usr/bin/env bash
# Package a universal (Apple Silicon + Intel) app into
# release/DownloadManagerPro-<version>-macos-universal.dmg
# Expects release builds for both aarch64-apple-darwin and x86_64-apple-darwin.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/core/Cargo.toml" | head -1)"
target="$root/core/target"
stage="$(mktemp -d)"
app="$stage/Download Manager Pro.app"

mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
for bin in dm-gui dm-daemon dm-native-host; do
    lipo -create \
        "$target/aarch64-apple-darwin/release/$bin" \
        "$target/x86_64-apple-darwin/release/$bin" \
        -output "$app/Contents/MacOS/$bin"
done

# App icon from the 256 px PNG.
iconset="$stage/AppIcon.iconset"
mkdir -p "$iconset"
png="$root/core/assets/app-icon.png"
for size in 16 32 128 256; do
    sips -z "$size" "$size" "$png" --out "$iconset/icon_${size}x${size}.png" >/dev/null
done
for size in 16 32 128; do
    double=$((size * 2))
    sips -z "$double" "$double" "$png" --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/AppIcon.icns"
rm -rf "$iconset"

cat > "$app/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>Download Manager Pro</string>
    <key>CFBundleDisplayName</key><string>Download Manager Pro</string>
    <key>CFBundleIdentifier</key><string>com.dmpro.downloadmanager</string>
    <key>CFBundleVersion</key><string>$version</string>
    <key>CFBundleShortVersionString</key><string>$version</string>
    <key>CFBundleExecutable</key><string>dm-gui</string>
    <key>CFBundleIconFile</key><string>AppIcon</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>LSMinimumSystemVersion</key><string>11.0</string>
    <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
EOF

# Ad-hoc signature: required for Apple Silicon. Not notarized, so Gatekeeper asks
# the user to confirm the first launch (right-click > Open).
codesign --force --deep --sign - "$app"

# The disk image: the app, a link to /Applications, and the browser extension folder.
dmg_root="$stage/dmg"
mkdir -p "$dmg_root"
mv "$app" "$dmg_root/"
ln -s /Applications "$dmg_root/Applications"
cp -r "$root/extension" "$dmg_root/Browser Extension"

mkdir -p "$root/release"
out="$root/release/DownloadManagerPro-$version-macos-universal.dmg"
hdiutil create -volname "Download Manager Pro $version" -srcfolder "$dmg_root" -ov -format UDZO "$out" >/dev/null
rm -rf "$stage"
echo "Packaged release/$(basename "$out")"
