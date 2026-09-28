#!/bin/bash
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
stage="$(mktemp -d "${TMPDIR:-/tmp}/surface-texture-events.XXXXXX")"
trap 'rm -rf -- "$stage"' EXIT

ndk_sysroot="${ANDROID_NDK_HOME:-$HOME/Library/Android/sdk/ndk/28.2.13676358}/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include"
xcrun clang++ -std=c++20 -Wall -Wextra -Werror \
  -fsanitize=address,undefined -Wno-nullability-completeness \
  -I"$root" -I"$root/compat" \
  -I"$root/tools/bionic-socket-broker-adapter/include" \
  -I"$root/_aosp/hwui-static-deps/frameworks-native/libs/nativedisplay/include" \
  -idirafter "$ndk_sysroot" \
  -I"$root/_build/nativehelper-foundation/source/libnativehelper/include_jni" \
  -I"$root/_aosp/android16-surfaceflinger-core/buffer-native/libs/nativewindow/include" \
  -I"$root/_aosp/android16-surfaceflinger-core/buffer-native/libs/nativebase/include" \
  -I"$root/_aosp/frameworks/native/libs/nativebase/include" \
  -I"$root/_aosp/system/core/libcutils/include" \
  -I"$root/_aosp/system/core/libsystem/include" \
  -I"$root/_aosp/frameworks/native/libs/arect/include" \
  "$root/tools/tests/surface-texture-events-test.cc" -o "$stage/test"

ASAN_OPTIONS=detect_leaks=0 UBSAN_OPTIONS=halt_on_error=1 "$stage/test"
