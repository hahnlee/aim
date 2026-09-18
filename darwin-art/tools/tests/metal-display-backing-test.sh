#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
test_dir="$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-metal-display-backing.XXXXXX")"
trap 'rm -rf "$test_dir"' EXIT
xcrun clang++ -std=c++20 -fobjc-arc -Wall -Wextra -Werror \
  -Wno-deprecated-declarations -fsanitize=address,undefined \
  -fno-omit-frame-pointer -I"$root" \
  "$root/compat/graphics/metal_display_backing.mm" \
  "$root/tools/tests/metal-display-backing-test.mm" \
  -framework Foundation -framework CoreFoundation -framework IOSurface \
  -framework Metal -o "$test_dir/test"
"$test_dir/test"
