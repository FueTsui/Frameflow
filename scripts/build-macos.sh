#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  printf '%s\n' 'Build the macOS application on a Mac with Xcode Command Line Tools.' >&2
  exit 1
fi

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET="${1:-$(rustc -vV | awk '/^host:/ {print $2}')}"
case "$TARGET" in
  aarch64-apple-darwin) ARCHITECTURE="arm64" ;;
  x86_64-apple-darwin) ARCHITECTURE="x64" ;;
  *) printf 'Unsupported macOS target: %s\n' "$TARGET" >&2; exit 1 ;;
esac

export MACOSX_DEPLOYMENT_TARGET="11.0"
cd "$PROJECT_ROOT"
cargo build --locked --release --target "$TARGET" --target-dir "$PROJECT_ROOT/target"
VERSION="$(awk -F '"' '/^version[[:space:]]*=/ {print $2; exit}' Cargo.toml)"
if [[ ! "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+([-+][A-Za-z0-9.-]+)?$ ]]; then
  printf '%s\n' 'Cannot read the package version from Cargo.toml.' >&2
  exit 1
fi
BUNDLE_VERSION="${VERSION%%[-+]*}"
PACKAGE_NAME="Frame-$VERSION-macos-$ARCHITECTURE"
mkdir -p "$PROJECT_ROOT/dist/.staging"
STAGE_ROOT="$(mktemp -d "$PROJECT_ROOT/dist/.staging/macos.XXXXXX")"
PACKAGE_ROOT="$STAGE_ROOT/$PACKAGE_NAME"
APP_ROOT="$PACKAGE_ROOT/Frame.app"
CONTENTS_ROOT="$APP_ROOT/Contents"
mkdir -p "$CONTENTS_ROOT/MacOS" "$CONTENTS_ROOT/Resources" "$CONTENTS_ROOT/MacOS/tools"
cp "$PROJECT_ROOT/target/$TARGET/release/frameflow" "$CONTENTS_ROOT/MacOS/frameflow"
chmod 755 "$CONTENTS_ROOT/MacOS/frameflow"
cp "$PROJECT_ROOT/README.md" "$PACKAGE_ROOT/README.md"
if [[ -d "$PROJECT_ROOT/docs/screenshots" ]]; then
  mkdir -p "$PACKAGE_ROOT/docs/screenshots"
  for screenshot in "$PROJECT_ROOT/docs/screenshots/"*.png; do
    if [[ -f "$screenshot" ]]; then
      cp "$screenshot" "$PACKAGE_ROOT/docs/screenshots/"
    fi
  done
fi
for license_name in LICENSE LICENSE-MIT LICENSE-APACHE THIRD_PARTY_NOTICES.md CHANGELOG.md; do
  if [[ -f "$PROJECT_ROOT/$license_name" ]]; then
    cp "$PROJECT_ROOT/$license_name" "$PACKAGE_ROOT/$license_name"
  fi
done

ICONSET_ROOT="$STAGE_ROOT/Frame.iconset"
mkdir -p "$ICONSET_ROOT"
sips -s format png "$PROJECT_ROOT/assets/Frameflow.ico" --out "$STAGE_ROOT/icon.png" >/dev/null
for icon_size in 16 32 128 256 512; do
  sips -z "$icon_size" "$icon_size" "$STAGE_ROOT/icon.png" --out "$ICONSET_ROOT/icon_${icon_size}x${icon_size}.png" >/dev/null
  retina_size=$((icon_size * 2))
  sips -z "$retina_size" "$retina_size" "$STAGE_ROOT/icon.png" --out "$ICONSET_ROOT/icon_${icon_size}x${icon_size}@2x.png" >/dev/null
done
iconutil --convert icns --output "$CONTENTS_ROOT/Resources/Frame.icns" "$ICONSET_ROOT"

cat > "$CONTENTS_ROOT/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Frameflow</string>
  <key>CFBundleDisplayName</key><string>帧流 Frameflow</string>
  <key>CFBundleIdentifier</key><string>app.frameflow.desktop</string>
  <key>CFBundleExecutable</key><string>frameflow</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleIconFile</key><string>Frame</string>
  <key>CFBundleShortVersionString</key><string>$BUNDLE_VERSION</string>
  <key>CFBundleVersion</key><string>$BUNDLE_VERSION</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.video</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST
plutil -lint "$CONTENTS_ROOT/Info.plist"
cat > "$CONTENTS_ROOT/MacOS/tools/README.txt" <<'TOOLS'
Place ffmpeg and ffprobe here, or select your FFmpeg folder in Frameflow settings.
Homebrew installations are also detected in /opt/homebrew/bin and /usr/local/bin.
Download links: https://ffmpeg.org/download.html
FFmpeg binaries are not included in this package.
TOOLS

# Preserve the executable bit and application bundle metadata inside the ZIP.
ARCHIVE_PATH="$PROJECT_ROOT/dist/$PACKAGE_NAME.zip"
ditto -c -k --sequesterRsrc --keepParent "$PACKAGE_ROOT" "$ARCHIVE_PATH"
printf 'Application: %s\nPackage: %s\n' "$APP_ROOT" "$ARCHIVE_PATH"
