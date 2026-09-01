#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
IOS_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
REPO_ROOT="$(cd "$IOS_DIR/../.." && pwd)"
BUILD_DIR="$(mktemp -d "${TMPDIR:-/tmp}/airferry-ios.XXXXXX")"
trap 'rm -rf "$BUILD_DIR"' EXIT

rustup target add aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios

cd "$REPO_ROOT"
cargo build -p transfer-engine --features cffi --release --target aarch64-apple-ios
cargo build -p transfer-engine --features cffi --release --target aarch64-apple-ios-sim
cargo build -p transfer-engine --features cffi --release --target x86_64-apple-ios

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

