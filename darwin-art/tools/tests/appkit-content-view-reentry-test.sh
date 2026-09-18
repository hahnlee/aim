#!/bin/bash
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
stage=$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-content-view-reentry.XXXXXX")
trap 'rm -rf "$stage"' EXIT
sdk=$(xcrun --sdk macosx --show-sdk-path)
# Link the real product class and surface destructor, not a second ObjC class
# implementation or a test replacement destructor. No APK/window is launched.
"${CXX:-clang++}" -arch arm64 -isysroot "$sdk" -std=c++20 -fobjc-arc -O1 -g \
  -Wall -Wextra -Werror -pthread -I"$root" \
  "$root/tools/tests/appkit-content-view-reentry-test.mm" \
  "$root/_build/runtime-graphics-link-probe/libdarwin_art_runtime_graphics.dylib" \
  -Wl,-rpath,"$root/_build/runtime-graphics-link-probe" \
  -framework AppKit -framework Metal -framework QuartzCore \
  -o "$stage/test"
"$stage/test"
