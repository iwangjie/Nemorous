#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
OUT_DIR="${1:-$ROOT_DIR/build/out}"
BIN_PATH="${2:-$ROOT_DIR/build/rust-target/release/nemorous}"
APP_DIR="$OUT_DIR/Nemorous.app"
VERSION="$(awk -F '"' '/^version = / {print $2; exit}' "$ROOT_DIR/Cargo.toml")"
if [[ ! "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "error: invalid bundle version in Cargo.toml" >&2
  exit 1
fi

if [[ ! -x "$BIN_PATH" ]]; then
  echo "error: release binary not found or not executable: $BIN_PATH" >&2
  exit 1
fi

rm -rf "$APP_DIR"
mkdir -p "$APP_DIR/Contents/MacOS" "$APP_DIR/Contents/Resources/nsh"

cp "$BIN_PATH" "$APP_DIR/Contents/MacOS/Nemorous"
chmod +x "$APP_DIR/Contents/MacOS/Nemorous"

cat > "$APP_DIR/Contents/Resources/nsh/app.json" <<JSON
{
  "name": "Nemorous",
  "id": "dev.nemorous.app",
  "version": "$VERSION",
  "appearance": "system",
  "window": {
    "title": "Nemorous",
    "width": 1440,
    "height": 900,
    "minWidth": 900,
    "minHeight": 580
  }
}
JSON

ICON_PLIST_LINE=""
if [[ -f "$ROOT_DIR/resources/icon.png" ]] && command -v sips >/dev/null; then
  ICONSET_DIR="$OUT_DIR/AppIcon.iconset"
  WORK_TMP="$OUT_DIR/.pkg-tmp"
  rm -rf "$ICONSET_DIR" "$WORK_TMP"
  mkdir -p "$ICONSET_DIR" "$WORK_TMP"
  for sz in 16 32 128 256 512; do
    TMPDIR="$WORK_TMP" sips -s format png -z "$sz" "$sz" "$ROOT_DIR/resources/icon.png" --out "$ICONSET_DIR/icon_${sz}x${sz}.png" >/dev/null
    sz2=$((sz * 2))
    TMPDIR="$WORK_TMP" sips -s format png -z "$sz2" "$sz2" "$ROOT_DIR/resources/icon.png" --out "$ICONSET_DIR/icon_${sz}x${sz}@2x.png" >/dev/null
  done
  if ! TMPDIR="$WORK_TMP" iconutil -c icns "$ICONSET_DIR" -o "$APP_DIR/Contents/Resources/AppIcon.icns" 2>/dev/null; then
    python3 - "$ICONSET_DIR" "$APP_DIR/Contents/Resources/AppIcon.icns" <<'PY'
import os, struct, sys
iconset, out = sys.argv[1], sys.argv[2]
mapping = [
    ("icp4", "icon_16x16.png"),
    ("ic11", "icon_16x16@2x.png"),
    ("icp5", "icon_32x32.png"),
    ("ic12", "icon_32x32@2x.png"),
    ("ic07", "icon_128x128.png"),
    ("ic13", "icon_128x128@2x.png"),
    ("ic08", "icon_256x256.png"),
    ("ic14", "icon_256x256@2x.png"),
    ("ic09", "icon_512x512.png"),
    ("ic10", "icon_512x512@2x.png"),
]
chunks = bytearray()
for tag, name in mapping:
    data = open(os.path.join(iconset, name), "rb").read()
    chunks += tag.encode("ascii") + struct.pack(">I", len(data) + 8) + data
with open(out, "wb") as f:
    f.write(b"icns" + struct.pack(">I", len(chunks) + 8) + chunks)
PY
  fi
  rm -rf "$ICONSET_DIR" "$WORK_TMP"
  ICON_PLIST_LINE="  <key>CFBundleIconFile</key>
  <string>AppIcon</string>"
fi

cat > "$APP_DIR/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key>
  <string>zh_CN</string>
  <key>CFBundleExecutable</key>
  <string>Nemorous</string>
  <key>CFBundleIdentifier</key>
  <string>dev.nemorous.app</string>
  <key>CFBundleName</key>
  <string>Nemorous</string>
  <key>CFBundleDisplayName</key>
  <string>Nemorous</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>$VERSION</string>
  <key>CFBundleVersion</key>
  <string>1</string>
  <key>CFBundleSupportedPlatforms</key>
  <array>
    <string>MacOSX</string>
  </array>
  <key>LSMinimumSystemVersion</key>
  <string>13.0</string>
  <key>NSHighResolutionCapable</key>
  <true/>
  <key>NSPrincipalClass</key>
  <string>NSApplication</string>
$ICON_PLIST_LINE
</dict>
</plist>
PLIST

codesign --force --sign - --timestamp=none "$APP_DIR"
codesign --verify --deep --strict "$APP_DIR"
echo "Packaged: $APP_DIR"
ls -lh "$APP_DIR/Contents/MacOS/Nemorous"
