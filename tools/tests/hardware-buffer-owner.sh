#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
ndk="${ANDROID_NDK_HOME:-$HOME/Library/Android/sdk/ndk/28.2.13676358}"
ndk_include="$ndk/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include"
[[ -f "$ndk_include/android/hardware_buffer.h" ]] || exit 2
stage="$(mktemp -d "${TMPDIR:-/tmp}/hardware-buffer-owner.XXXXXX")"
trap 'rm -rf -- "$stage"' EXIT
sdk="$(xcrun --sdk macosx --show-sdk-path)"
cxx="$(xcrun --find clang++)"
# Existing handle transport uses deprecated global IOSurface IDs. Preserve the
# warning without changing that IPC contract as part of this owner extraction.
"$cxx" -arch arm64 -isysroot "$sdk" -std=c++20 -O1 -g \
  -Wall -Wextra -Werror -Wno-nullability-completeness -pthread \
  -Wno-error=deprecated-declarations \
  -fsanitize=address,undefined -fno-omit-frame-pointer \
  -I"$root" -I"$root/tools/bionic-socket-broker-adapter/include" \
  -idirafter "$ndk_include" \
  "$root/compat/graphics/hardware_buffer_owner.mm" \
  "$root/tools/tests/hardware-buffer-owner-test.mm" \
  -framework Foundation -framework IOSurface -framework Metal \
  -o "$stage/hardware-buffer-owner-test"
ASAN_OPTIONS=detect_leaks=0:halt_on_error=1 \
UBSAN_OPTIONS=halt_on_error=1 "$stage/hardware-buffer-owner-test"
