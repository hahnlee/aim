#!/bin/bash
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
ndk="${ANDROID_NDK_HOME:-$HOME/Library/Android/sdk/ndk/28.2.13676358}"
stage="$(mktemp -d "${TMPDIR:-/tmp}/native-window-software-queue.XXXXXX")"
trap 'rm -rf -- "$stage"' EXIT

sdk="$(xcrun --sdk macosx --show-sdk-path)"
cxx="$(xcrun --find clang++)"
sanitizers="${SANITIZER:-address,undefined}"
flags=(
  -arch arm64
  -isysroot "$sdk"
  -std=c++20
  -O1
  -g
  -Wall
  -Wextra
  -Werror
  -Wno-nullability-completeness
  "-fsanitize=$sanitizers"
  -fno-omit-frame-pointer
  -I"$root"
  -I"$root/compat"
  -I"$root/_aosp/android16-surfaceflinger-core/buffer-native/libs/nativewindow/include"
  -I"$root/_aosp/android16-surfaceflinger-core/buffer-native/libs/nativebase/include"
  -I"$root/_aosp/frameworks/native/libs/nativebase/include"
  -I"$root/_aosp/system/core/libcutils/include"
  -I"$root/_aosp/system/core/libsystem/include"
  -I"$root/_aosp/frameworks/native/libs/arect/include"
  -idirafter "$ndk/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include"
)

"$cxx" "${flags[@]}" -c \
  "$root/compat/window/native_window_software_queue.cc" \
  -o "$stage/queue.o"
"$cxx" "${flags[@]}" -c \
  "$root/tools/tests/native-window-software-queue-test.cc" \
  -o "$stage/test.o"
"$cxx" "${flags[@]}" "$stage/queue.o" "$stage/test.o" \
  -o "$stage/test"

ASAN_OPTIONS=detect_leaks=0:halt_on_error=1 \
UBSAN_OPTIONS=halt_on_error=1 \
  "$stage/test"
