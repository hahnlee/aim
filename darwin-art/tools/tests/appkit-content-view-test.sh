#!/bin/bash
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
stage=$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-content-view.XXXXXX")
trap 'rm -rf "$stage"' EXIT
sdk=$(xcrun --sdk macosx --show-sdk-path)
"${CXX:-clang++}" -arch arm64 -isysroot "$sdk" -std=c++20 -fobjc-arc -O1 -g \
  -Wall -Wextra -Werror -pthread -I"$root" \
  "$root/compat/window/appkit_content_view.mm" \
  "$root/compat/graphics/surface_backing_owner.mm" \
  "$root/compat/input/darwin_hardware_key_translation.mm" \
  "$root/tools/tests/appkit-content-view-test.mm" \
  -framework AppKit -framework IOSurface -framework Metal -framework QuartzCore \
  -o "$stage/test"
"$stage/test"
