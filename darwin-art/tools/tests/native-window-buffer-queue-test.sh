#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
ndk="${ANDROID_NDK_HOME:-$HOME/Library/Android/sdk/ndk/28.2.13676358}"
include="$ndk/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include"
stage="$(mktemp -d "${TMPDIR:-/tmp}/native-window-buffer-queue.XXXXXX")"
trap 'rm -rf -- "$stage"' EXIT
sdk="$(xcrun --sdk macosx --show-sdk-path)"
xcrun clang++ -arch arm64 -isysroot "$sdk" -std=c++20 -O1 -g \
  -Wall -Wextra -Werror -Wno-nullability-completeness \
  -fsanitize=address,undefined -fno-omit-frame-pointer \
  -I"$root" -idirafter "$include" \
  "$root/compat/window/native_window_buffer_queue.cc" \
  "$root/tools/tests/native-window-buffer-queue-test.cc" -o "$stage/test"
ASAN_OPTIONS=detect_leaks=0:halt_on_error=1 UBSAN_OPTIONS=halt_on_error=1 "$stage/test"
