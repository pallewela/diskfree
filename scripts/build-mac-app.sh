#!/bin/sh
# Build the native macOS app (macOS 26+) as target/Diskfree.app.
# A real bundle lets you grant it Full Disk Access and launch it from Finder.
# --dmg also packs it into target/Diskfree-<version>.dmg (drag to Applications).
set -eu
cd "$(dirname "$0")/.."

cargo build --release --lib
swift build -c release --package-path macos

VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
APP=target/Diskfree.app
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp macos/.build/release/Diskfree "$APP/Contents/MacOS/Diskfree"
cp icon/Diskfree.icns "$APP/Contents/Resources/Diskfree.icns"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key><string>Diskfree</string>
    <key>CFBundleIdentifier</key><string>com.github.pallewela.diskfree</string>
    <key>CFBundleName</key><string>diskfree</string>
    <key>CFBundleDisplayName</key><string>diskfree</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleIconFile</key><string>Diskfree</string>
    <key>CFBundleShortVersionString</key><string>$VERSION</string>
    <key>CFBundleVersion</key><string>$VERSION</string>
    <key>LSMinimumSystemVersion</key><string>26.0</string>
    <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST
# Ad-hoc signature so macOS keeps privacy grants (Full Disk Access) stable for this build.
codesign --force --sign - "$APP"
echo "Built $APP"

if [ "${1:-}" = "--dmg" ]; then
    # ponytail: plain Finder window, no background art or icon layout; add create-dmg if that matters.
    STAGE=target/dmg
    DMG=target/Diskfree-$VERSION.dmg
    rm -rf "$STAGE" "$DMG"
    mkdir -p "$STAGE"
    cp -R "$APP" "$STAGE/"
    ln -s /Applications "$STAGE/Applications"
    hdiutil create -volname diskfree -srcfolder "$STAGE" -format UDZO -fs APFS -ov "$DMG" >/dev/null
    rm -rf "$STAGE"
    echo "Built $DMG"
fi
