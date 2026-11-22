#!/usr/bin/env bash
# Package the release binary as an ad-hoc-signed Relay.app. Run on macOS after
# `cargo build --release -p relay-ui`.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
target="${CARGO_TARGET_DIR:-$root/target}/release/relay"
app="$root/dist/Relay.app"
version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)"

if [[ ! -x "$target" ]]; then
  echo "bundle: missing $target; run cargo build --release -p relay-ui first" >&2
  exit 1
fi

rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$target" "$app/Contents/MacOS/relay"

if [[ -f "$root/assets/AppIcon.icns" ]]; then
  cp "$root/assets/AppIcon.icns" "$app/Contents/Resources/AppIcon.icns"
  icon='<key>CFBundleIconFile</key><string>AppIcon</string>'
else
  icon=''
fi

cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Relay</string>
  <key>CFBundleDisplayName</key><string>Relay</string>
  <key>CFBundleIdentifier</key><string>dev.relay.browser</string>
  <key>CFBundleExecutable</key><string>relay</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>${version:-0.1.0}</string>
  <key>CFBundleVersion</key><string>${version:-0.1.0}</string>
  <key>LSMinimumSystemVersion</key><string>13.3</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSPrincipalClass</key><string>NSApplication</string>
  <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
  <!-- A browser must reach plain-http sites; WebKit still enforces TLS rules for apps. -->
  <key>NSAppTransportSecurity</key>
  <dict><key>NSAllowsArbitraryLoadsInWebContent</key><true/></dict>
  ${icon:+$icon}
</dict>
</plist>
PLIST

# Apple Silicon requires a signature even for local runs; ad-hoc is enough here.
codesign --force --deep --sign - "$app" >/dev/null
codesign --verify --deep --strict "$app"

"$app/Contents/MacOS/relay" --version
echo "bundle: $app"
