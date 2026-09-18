#!/bin/bash
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
ndk="${ANDROID_NDK_HOME:-$HOME/Library/Android/sdk/ndk/28.2.13676358}"
ndk_include="$ndk/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include"
[[ -f "$ndk_include/android/hardware_buffer.h" ]] || exit 2
stage="$(mktemp -d "${TMPDIR:-/tmp}/surface-control-submit-darwin.XXXXXX")"
trap 'rm -rf -- "$stage"' EXIT
sdk="$(xcrun --sdk macosx --show-sdk-path)"
cxx="$(xcrun --find clang++)"
"$cxx" -arch arm64 -isysroot "$sdk" -std=c++20 -O1 -g \
  -Wall -Wextra -Werror -Wno-nullability-completeness -pthread \
  -I"$root" -I"$root/compat" -I"$root/include" \
  -I"$root/_aosp/frameworks/native/libs/nativewindow/include" \
  -I"$root/_aosp/frameworks/native/libs/arect/include" \
  -I"$root/_aosp/system/core/libsystem/include" \
  -idirafter "$ndk_include" \
  "$root/compat/window/surface_control_submit_darwin.mm" \
  "$root/compat/window/surface_control_state.cc" \
  "$root/tools/tests/surface-control-submit-darwin-test.cc" \
  -framework CoreFoundation -framework IOSurface \
  -o "$stage/surface-control-submit-darwin-test"
"$stage/surface-control-submit-darwin-test"
