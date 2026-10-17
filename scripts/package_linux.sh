#!/usr/bin/env bash
# Package the Linux build into release/DownloadManagerPro-<version>-linux-<arch>.tar.gz
# Expects `cargo build --release --bin dm-gui --bin dm-daemon --bin dm-native-host` to have run.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/core/Cargo.toml" | head -1)"
arch="$(uname -m)"
bin_dir="$root/core/target/release"
name="DownloadManagerPro-$version-linux-$arch"
stage_root="$(mktemp -d)"
stage="$stage_root/$name"

mkdir -p "$stage"
for bin in dm-gui dm-daemon dm-native-host; do
    install -m 755 "$bin_dir/$bin" "$stage/$bin"
done
cp -r "$root/extension" "$stage/browser-extension"
cp "$root/LICENSE" "$stage/"
cp "$root/core/assets/app-icon.png" "$stage/download-manager-pro.png"
install -m 755 "$root/packaging/linux/install.sh" "$stage/install.sh"

mkdir -p "$root/release"
tar -C "$stage_root" -czf "$root/release/$name.tar.gz" "$name"
rm -rf "$stage_root"
echo "Packaged release/$name.tar.gz"
