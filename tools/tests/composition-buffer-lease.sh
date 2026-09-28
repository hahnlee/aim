#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
ndk="${ANDROID_NDK_HOME:-$HOME/Library/Android/sdk/ndk/28.2.13676358}"
ndk_include="$ndk/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include"
[[ -f "$ndk_include/android/hardware_buffer.h" ]] || exit 2
stage="$(mktemp -d "${TMPDIR:-/tmp}/composition-buffer-lease.XXXXXX")"
trap 'rm -rf -- "$stage"' EXIT
sdk="$(xcrun --sdk macosx --show-sdk-path)"
cxx="$(xcrun --find clang++)"
"$cxx" -arch arm64 -isysroot "$sdk" -std=c++20 -O1 -g \
  -Wall -Wextra -Werror -Wno-nullability-completeness -pthread \
  -I"$root" -idirafter "$ndk_include" \
  "$root/compat/graphics/composition_buffer_lease.cc" \
  "$root/tools/tests/composition-buffer-lease-test.cc" \
  -o "$stage/composition-buffer-lease-test"
"$stage/composition-buffer-lease-test"
