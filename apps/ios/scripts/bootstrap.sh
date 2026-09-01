#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
IOS_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

if ! command -v rustup >/dev/null 2>&1 || ! command -v cargo >/dev/null 2>&1; then
  echo "Rust is required. Install it from https://rustup.rs and run this script again." >&2
  exit 1
fi
if ! command -v xcodegen >/dev/null 2>&1; then
  echo "XcodeGen is required. Install it with: brew install xcodegen" >&2
  exit 1
fi

"$SCRIPT_DIR/build-rust-xcframework.sh"
cd "$IOS_DIR"
xcodegen generate
echo "Open $IOS_DIR/AirFerryIOS.xcodeproj, select your signing team, and run on a real device."

