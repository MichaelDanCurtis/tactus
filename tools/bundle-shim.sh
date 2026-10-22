#!/bin/sh
# Build the shim and package it as a VST3 bundle.
#
#   tools/bundle-shim.sh [release|debug] [--install]
#
# Writes target/<profile>/Tactus Shim.vst3. With --install, copies it to
# ~/Library/Audio/Plug-Ins/VST3, where DAWs look for user plug-ins.
set -eu
profile="${1:-release}"
cd "$(dirname "$0")/.."
if [ "$profile" = release ]; then
    cargo build -q --release -p tactus-shim
else
    cargo build -q -p tactus-shim
fi
out="target/$profile/Tactus Shim.vst3"
rm -rf "$out"
mkdir -p "$out/Contents/MacOS" "$out/Contents/Resources"
cp "target/$profile/libtactus_shim.dylib" "$out/Contents/MacOS/Tactus Shim"
version=$(sed -n 's/^version = "\(.*\)"/\1/p' crates/tactus-shim/Cargo.toml | head -1)
cat > "$out/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key><string>English</string>
    <key>CFBundleExecutable</key><string>Tactus Shim</string>
    <key>CFBundleIdentifier</key><string>dev.tactus.shim</string>
    <key>CFBundleName</key><string>Tactus Shim</string>
    <key>CFBundlePackageType</key><string>BNDL</string>
    <key>CFBundleShortVersionString</key><string>$version</string>
    <key>CFBundleVersion</key><string>$version</string>
    <key>CFBundleSignature</key><string>????</string>
</dict>
</plist>
PLIST
printf 'BNDL????' > "$out/Contents/PkgInfo"
codesign --force --sign - "$out" >/dev/null 2>&1 || true
echo "$out"
if [ "${2:-}" = --install ]; then
    dest="$HOME/Library/Audio/Plug-Ins/VST3/Tactus Shim.vst3"
    rm -rf "$dest"
    mkdir -p "$HOME/Library/Audio/Plug-Ins/VST3"
    cp -R "$out" "$dest"
    echo "installed: $dest"
fi
