#!/bin/bash
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
out="$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-surface-backing-owner.XXXXXX")"
trap 'rm -rf "$out"' EXIT

clang="$(xcrun --find clang++)"
sdk="$(xcrun --sdk macosx --show-sdk-path)"
sources=("$root/compat/graphics/surface_backing_owner.mm"
         "$root/compat/graphics/metal_display_backing.mm")
if [[ "${1:-}" == --production-objects ]]; then
  sources=("$root/_build/runtime-common/objects/graphics_surface_backing_owner.mm.o"
           "$root/_build/runtime-common/objects/graphics_metal_display_backing.mm.o")
elif [[ $# != 0 ]]; then
  echo 'usage: surface-backing-owner-test.sh [--production-objects]' >&2; exit 64
fi
"$clang" -arch arm64 -isysroot "$sdk" -std=c++20 -fobjc-arc \
  -Wall -Wextra -Werror -Wno-deprecated-declarations -fsanitize=address,undefined \
  -fno-omit-frame-pointer -pthread -I"$root/compat" \
  "$root/tools/tests/surface-backing-owner-test.mm" \
  "${sources[@]}" \
  -framework Foundation -framework IOSurface -framework Metal \
  -o "$out/surface-backing-owner-test"
"$out/surface-backing-owner-test"
echo "surface-backing-owner-test: pass"
