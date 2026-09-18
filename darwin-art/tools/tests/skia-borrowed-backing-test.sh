#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
stage="$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-skia-borrowed.XXXXXX")"
trap 'rm -rf -- "$stage"' EXIT
xcrun clang++ -std=c++20 -fobjc-arc -Wall -Wextra -Werror \
  -Wno-deprecated-declarations -I"$root" -I"$root/_aosp/external/skia" \
  -I"$root/_aosp/system/logging/liblog/include" \
  '-DSK_USER_CONFIG_HEADER="include/config/SkUserConfigManual.h"' \
  "$root/tools/tests/skia-borrowed-backing-test.mm" \
  "$root/compat/graphics/surface_backing_owner.mm" \
  "$root/compat/graphics/metal_display_backing.mm" \
  "$root/_build/skia-metal-gpu/libskia.a" "$root/_build/skia-metal-gpu/libskcms.a" \
  "$root/_build/graphics-codecs/libft2-darwin.a" \
  "$root/_build/graphics-codecs/libpng-darwin.a" \
  "$root/_build/graphics-codecs/libz-darwin.a" \
  "$root/_build/codec-foundation/libjpeg-darwin.a" \
  "$root/_build/graphics-foundations/liblog-darwin.a" \
  -framework Foundation -framework CoreFoundation -framework Metal \
  -framework IOSurface -framework QuartzCore -framework CoreGraphics \
  -framework CoreText -o "$stage/test"
"$stage/test"
