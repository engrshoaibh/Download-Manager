#!/usr/bin/env bash
# Install Download Manager Pro for the current user (no root needed).
#   ./install.sh               install (or upgrade) and start the app
#   ./install.sh --autostart   also start it when you log in
#   ./install.sh --uninstall   remove it
set -euo pipefail

src="$(cd "$(dirname "$0")" && pwd)"
dest="${XDG_DATA_HOME:-$HOME/.local/share}/download-manager-pro"
apps="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
icons="${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor/256x256/apps"
autostart="${XDG_CONFIG_HOME:-$HOME/.config}/autostart"
bin_link="$HOME/.local/bin/download-manager-pro"
desktop_file="download-manager-pro.desktop"
config="${XDG_CONFIG_HOME:-$HOME/.config}"
host_json="NativeMessagingHosts/com.dmpro.native_host.json"

if [[ "${1:-}" == "--uninstall" ]]; then
    pkill -f "$dest/dm-gui" 2>/dev/null || true
    rm -rf "$dest"
    rm -f "$apps/$desktop_file" "$autostart/$desktop_file" "$icons/download-manager-pro.png" "$bin_link"
    for b in google-chrome google-chrome-beta chromium microsoft-edge BraveSoftware/Brave-Browser vivaldi; do
        rm -f "$config/$b/$host_json"
    done
    echo "Download Manager Pro removed. Your downloads and settings in ~/.local/share/download-manager were kept."
    exit 0
fi

# Replace a running copy so the new binaries can be written.
pkill -f "$dest/dm-gui" 2>/dev/null || true
mkdir -p "$dest" "$apps" "$icons" "$(dirname "$bin_link")"
cp -f "$src/dm-gui" "$src/dm-daemon" "$src/dm-native-host" "$dest/"
rm -rf "$dest/browser-extension"
cp -r "$src/browser-extension" "$dest/browser-extension"
cp -f "$src/LICENSE" "$dest/"
cp -f "$src/download-manager-pro.png" "$icons/"
ln -sf "$dest/dm-gui" "$bin_link"

cat > "$apps/$desktop_file" <<EOF
[Desktop Entry]
Type=Application
Name=Download Manager Pro
Comment=Fast multi-connection download manager
Exec="$dest/dm-gui"
Icon=download-manager-pro
Categories=Network;FileTransfer;
Terminal=false
EOF

if [[ "${1:-}" == "--autostart" ]]; then
    mkdir -p "$autostart"
    cp -f "$apps/$desktop_file" "$autostart/$desktop_file"
fi
command -v update-desktop-database >/dev/null && update-desktop-database "$apps" >/dev/null 2>&1 || true

# The app registers its browser native-messaging host each time it starts.
nohup "$dest/dm-gui" >/dev/null 2>&1 &

cat <<EOF
Download Manager Pro is installed in $dest and starting now.

Browser extension: open chrome://extensions (or edge://extensions), turn on
Developer mode, choose "Load unpacked" and select:
    $dest/browser-extension
EOF
