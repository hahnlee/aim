#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
stage="$(mktemp -d "${TMPDIR:-/tmp}/egl-native-fence-owner.XXXXXX")"
trap 'rm -rf -- "$stage"' EXIT

sdk="$(xcrun --sdk macosx --show-sdk-path)"
cxx="$(xcrun --find clang++)"
flags=(
  -arch arm64
  -isysroot "$sdk"
  -std=c++20
  -O1
  -g
  -Wall
  -Wextra
  -Werror
  -DDARWIN_ART_EGL_NATIVE_FENCE_OWNER_TESTING
  -fsanitize=address,undefined
  -fno-omit-frame-pointer
  -I"$root"
  -pthread
)

"$cxx" "${flags[@]}" -c "$root/compat/graphics/egl_native_fence_owner.cc" \
  -o "$stage/owner.o"
"$cxx" "${flags[@]}" -c \
  "$root/tools/tests/egl-native-fence-owner-test.cc" -o "$stage/test.o"
"$cxx" "${flags[@]}" "$stage/test.o" "$stage/owner.o" \
  -o "$stage/egl-native-fence-owner-test"

ASAN_OPTIONS=detect_leaks=0:halt_on_error=1 \
UBSAN_OPTIONS=halt_on_error=1 "$stage/egl-native-fence-owner-test"
