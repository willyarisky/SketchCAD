#!/usr/bin/env bash
# Build SketchCAD for macOS (universal .app), Windows x86_64 and Linux x86_64/arm64 into dist/.
#
# Run on a Mac. One-time setup:
#   brew install zig mingw-w64
#   cargo install cargo-zigbuild --locked
#   rustup target add aarch64-apple-darwin x86_64-apple-darwin x86_64-pc-windows-gnu \
#                     x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu
set -euo pipefail
cd "$(dirname "$0")/.."

# rustup keeps cargo in ~/.cargo/bin and puts it on PATH from your shell profile. Running this
# with `bash build-release.sh` from zsh (or from a cron job, or an IDE) skips that profile, so
# pick the toolchain up the way rustup's own installer does.
if ! command -v cargo >/dev/null 2>&1 && [ -r "$HOME/.cargo/env" ]; then
  . "$HOME/.cargo/env"
fi

# Report everything that is missing at once: finding out about mingw after the macOS build has
# already run is a slow way to learn it.
missing=()
for tool in cargo rustup lipo codesign zip ditto tar shasum; do
  command -v "$tool" >/dev/null 2>&1 || missing+=("$tool")
done
if command -v cargo >/dev/null 2>&1; then
  # Checked as a binary on PATH: `cargo zigbuild` has no --version flag to probe with.
  command -v cargo-zigbuild >/dev/null 2>&1 || missing+=("cargo-zigbuild (cargo install cargo-zigbuild --locked)")
  command -v zig >/dev/null 2>&1 || missing+=("zig (brew install zig)")
  command -v x86_64-w64-mingw32-gcc >/dev/null 2>&1 || missing+=("mingw-w64 (brew install mingw-w64)")
  for t in aarch64-apple-darwin x86_64-apple-darwin x86_64-pc-windows-gnu x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu; do
    rustup target list --installed 2>/dev/null | grep -qx "$t" || missing+=("rust target $t (rustup target add $t)")
  done
fi
if [ ${#missing[@]} -gt 0 ]; then
  echo "Cannot build the release; these are missing:" >&2
  printf '  - %s\n' "${missing[@]}" >&2
  [ -d "$HOME/.cargo/bin" ] && ! command -v cargo >/dev/null 2>&1 &&
    echo "(cargo is in ~/.cargo/bin but not on PATH — see the note at the top of this script)" >&2
  exit 1
fi

VERSION=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
DIST=dist
GLIBC=2.31   # oldest glibc the Linux builds run on (Ubuntu 20.04, Debian 11)
PKGS=(-p sc_app -p sc_skp)
rm -rf "$DIST" && mkdir -p "$DIST"

common_files() {   # $1 = destination folder
  cp README.md "$1/"
  mkdir -p "$1/examples"
  cp examples/extensions/box_maker.rbz "$1/examples/"
}

echo "==> macOS (arm64 + x86_64)"
cargo build --release --target aarch64-apple-darwin "${PKGS[@]}"
cargo build --release --target x86_64-apple-darwin "${PKGS[@]}"
APP="$DIST/SketchCAD.app"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
lipo -create -output "$APP/Contents/MacOS/sketchcad" \
  target/aarch64-apple-darwin/release/sketchcad target/x86_64-apple-darwin/release/sketchcad
lipo -create -output "$APP/Contents/MacOS/sketchcad-skp" \
  target/aarch64-apple-darwin/release/sketchcad-skp target/x86_64-apple-darwin/release/sketchcad-skp
# Intel-only SketchUp libraries (SketchUp 2020 and older) need an Intel converter, run under Rosetta.
cp target/x86_64-apple-darwin/release/sketchcad-skp "$APP/Contents/MacOS/sketchcad-skp-x86_64"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>SketchCAD</string>
  <key>CFBundleDisplayName</key><string>SketchCAD</string>
  <key>CFBundleIdentifier</key><string>app.sketchcad.SketchCAD</string>
  <key>CFBundleExecutable</key><string>sketchcad</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST
# Optionally ship Trimble's SketchUpAPI inside the app, so .skp files open with nothing installed.
# Point SKETCHUP_SDK_MAC at an unpacked macOS SDK folder holding SketchUpAPI.framework.
# Check Trimble's SDK licence before redistributing it.
if [ -n "${SKETCHUP_SDK_MAC:-}" ]; then
  [ -d "$SKETCHUP_SDK_MAC/SketchUpAPI.framework" ] || { echo "no SketchUpAPI.framework in $SKETCHUP_SDK_MAC" >&2; exit 1; }
  mkdir -p "$APP/Contents/Frameworks"
  cp -R "$SKETCHUP_SDK_MAC/SketchUpAPI.framework" "$APP/Contents/Frameworks/"
fi
codesign --force --deep --sign - "$APP"   # ad-hoc signature (not notarized)
MAC="$DIST/SketchCAD-$VERSION-macos-universal"
mkdir -p "$MAC" && mv "$APP" "$MAC/" && common_files "$MAC"
(cd "$DIST" && ditto -c -k --keepParent "$(basename "$MAC")" "$(basename "$MAC").zip")

echo "==> Windows x86_64"
cargo build --release --target x86_64-pc-windows-gnu "${PKGS[@]}"
WIN="$DIST/SketchCAD-$VERSION-windows-x86_64"
mkdir -p "$WIN"
cp target/x86_64-pc-windows-gnu/release/sketchcad.exe target/x86_64-pc-windows-gnu/release/sketchcad-skp.exe "$WIN/"
# As above: SKETCHUP_SDK_WIN is a folder holding SketchUpAPI.dll (and the DLLs it needs).
if [ -n "${SKETCHUP_SDK_WIN:-}" ]; then
  [ -f "$SKETCHUP_SDK_WIN/SketchUpAPI.dll" ] || { echo "no SketchUpAPI.dll in $SKETCHUP_SDK_WIN" >&2; exit 1; }
  cp "$SKETCHUP_SDK_WIN"/*.dll "$WIN/"
fi
common_files "$WIN"
(cd "$DIST" && zip -qr "$(basename "$WIN").zip" "$(basename "$WIN")")

for arch in x86_64 aarch64; do
  echo "==> Linux $arch"
  cargo zigbuild --release --target "$arch-unknown-linux-gnu.$GLIBC" "${PKGS[@]}"
  LNX="$DIST/SketchCAD-$VERSION-linux-$arch"
  mkdir -p "$LNX"
  cp "target/$arch-unknown-linux-gnu/release/sketchcad" "target/$arch-unknown-linux-gnu/release/sketchcad-skp" "$LNX/"
  common_files "$LNX"
  tar -C "$DIST" -czf "$LNX.tar.gz" "$(basename "$LNX")"
done

(cd "$DIST" && shasum -a 256 ./*.zip ./*.tar.gz > SHA256SUMS.txt)
ls -lh "$DIST"
