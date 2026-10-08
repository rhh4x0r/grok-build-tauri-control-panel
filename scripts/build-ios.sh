#!/usr/bin/env bash
# Build the iOS companion's Rust core and its Swift bindings.
# Output: ios/BombMobile.xcframework (device + simulator) and ios/BombCode/Generated/bomb_mobile.swift
# Usage: scripts/build-ios.sh [debug|release]   (default: release)
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

PROFILE="${1:-release}"
FLAG=$([ "$PROFILE" = "release" ] && echo "--release" || echo "")
DEVICE=aarch64-apple-ios
SIM=aarch64-apple-ios-sim

for target in "$DEVICE" "$SIM"; do
  cargo build $FLAG -p bomb_mobile --lib --target "$target"
done

GEN="${ROOT}/target/ios-bindings"
rm -rf "$GEN"
cargo run -q -p bomb_mobile --features bindgen --bin uniffi-bindgen -- \
  generate --library "target/${DEVICE}/${PROFILE}/libbomb_mobile.a" --language swift --out-dir "$GEN"

# The Swift file goes into the app; the C header and module map go inside the framework.
mkdir -p "${ROOT}/ios/BombCode/Generated"
cp "${GEN}/bomb_mobile.swift" "${ROOT}/ios/BombCode/Generated/"
HEADERS="${GEN}/headers"
mkdir -p "$HEADERS"
cp "${GEN}/bomb_mobileFFI.h" "$HEADERS/"
cp "${GEN}/bomb_mobileFFI.modulemap" "$HEADERS/module.modulemap"

OUT="${ROOT}/ios/BombMobile.xcframework"
rm -rf "$OUT"
xcodebuild -create-xcframework \
  -library "target/${DEVICE}/${PROFILE}/libbomb_mobile.a" -headers "$HEADERS" \
  -library "target/${SIM}/${PROFILE}/libbomb_mobile.a" -headers "$HEADERS" \
  -output "$OUT"
echo "Built $OUT"
