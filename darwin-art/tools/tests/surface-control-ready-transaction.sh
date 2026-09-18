#!/bin/bash
set -euo pipefail
test_root="$(cd "$(dirname "$0")/../.." && pwd)"
ndk_root="${ANDROID_NDK_HOME:-$HOME/Library/Android/sdk/ndk/28.2.13676358}"
ndk_include="$ndk_root/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include"
[[ -f "$ndk_include/android/hardware_buffer.h" ]] || exit 2
stage="$(mktemp -d "${TMPDIR:-/tmp}/surface-control-ready.XXXXXX")"
trap 'rm -rf -- "$stage"' EXIT
sdk_path="$(xcrun --sdk macosx --show-sdk-path)"
cxx_path="$(xcrun --find clang++)"
"$cxx_path" -arch arm64 -isysroot "$sdk_path" -std=c++20 -O1 -g \
  -Wall -Wextra -Werror -Wno-nullability-completeness -pthread \
  -I"$test_root" -I"$test_root/compat" -I"$test_root/include" \
  -I"$test_root/_aosp/frameworks/native/libs/nativewindow/include" \
  -I"$test_root/_aosp/frameworks/native/libs/arect/include" \
  -I"$test_root/_aosp/system/core/libsystem/include" \
  -idirafter "$ndk_include" \
  "$test_root/compat/window/surface_control_ready_transaction.cc" \
  "$test_root/compat/window/surface_control_registry.cc" \
  "$test_root/compat/window/surface_control_state.cc" \
  "$test_root/tools/tests/surface-control-ready-transaction-test.cc" \
  -o "$stage/surface-control-ready-transaction-test"
"$stage/surface-control-ready-transaction-test"
