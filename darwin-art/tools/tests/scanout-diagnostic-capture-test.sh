#!/bin/bash
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
out="$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-scanout-capture.XXXXXX")"
trap 'rm -rf "$out"' EXIT
clang="$(xcrun --find clang++)"
sdk="$(xcrun --sdk macosx --show-sdk-path)"
"$clang" -arch arm64 -isysroot "$sdk" -std=c++20 -fobjc-arc \
  -Wall -Wextra -Werror -Wno-deprecated-declarations -fsanitize=address,undefined \
  -fno-omit-frame-pointer -pthread -I"$root/compat" \
  "$root/tools/tests/scanout-diagnostic-capture-test.mm" \
  "$root/compat/graphics/scanout_diagnostic_capture.mm" \
  "$root/compat/graphics/surface_backing_owner.mm" \
  "$root/compat/graphics/metal_display_backing.mm" \
  -framework AppKit -framework QuartzCore -framework IOSurface -framework Metal \
  -o "$out/test"
"$out/test" disabled "$out/disabled"
"$out/test" enabled "$out/frame"
"$out/test" failure "$out/missing/frame" 2>"$out/failure.log"
rg 'written=0' "$out/failure.log"
echo 'scanout-diagnostic-capture-test: pass'
