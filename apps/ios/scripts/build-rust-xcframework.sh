#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
IOS_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
REPO_ROOT="$(cd "$IOS_DIR/../.." && pwd)"
BUILD_DIR="$(mktemp -d "${TMPDIR:-/tmp}/airferry-ios.XXXXXX")"
trap 'rm -rf "$BUILD_DIR"' EXIT
IOS_DEPLOYMENT_TARGET="${IPHONEOS_DEPLOYMENT_TARGET:-17.0}"

rustup target add aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios

cd "$REPO_ROOT"

# The app embeds a static archive. Override the package's normal multi-platform
# crate types so Cargo does not also try to link an iOS dylib. Keep Rust and all
# native C dependencies on the same deployment target as the Xcode project.
build_staticlib() {
  local target="$1"
  local cflags="$2"

  IPHONEOS_DEPLOYMENT_TARGET="$IOS_DEPLOYMENT_TARGET" \
    CFLAGS="$cflags" \
    cargo rustc \
      -p transfer-engine \
      --lib \
      --crate-type staticlib \
      --features cffi \
      --release \
      --target "$target"
}

build_staticlib aarch64-apple-ios \
  "-miphoneos-version-min=$IOS_DEPLOYMENT_TARGET"
build_staticlib aarch64-apple-ios-sim \
  "-mios-simulator-version-min=$IOS_DEPLOYMENT_TARGET"
build_staticlib x86_64-apple-ios \
  "-mios-simulator-version-min=$IOS_DEPLOYMENT_TARGET"

cp "$REPO_ROOT/target/aarch64-apple-ios/release/libtransfer_engine.a" \
  "$BUILD_DIR/libAirFerryCore-device.a"
lipo -create \
  "$REPO_ROOT/target/aarch64-apple-ios-sim/release/libtransfer_engine.a" \
  "$REPO_ROOT/target/x86_64-apple-ios/release/libtransfer_engine.a" \
  -output "$BUILD_DIR/libAirFerryCore-simulator.a"

OUTPUT="$IOS_DIR/Native/AirFerryCore.xcframework"
rm -rf "$OUTPUT"
xcodebuild -create-xcframework \
  -library "$BUILD_DIR/libAirFerryCore-device.a" -headers "$IOS_DIR/Native/include" \
  -library "$BUILD_DIR/libAirFerryCore-simulator.a" -headers "$IOS_DIR/Native/include" \
  -output "$OUTPUT"

echo "Created $OUTPUT"
