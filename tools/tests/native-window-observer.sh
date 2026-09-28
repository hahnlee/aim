#!/bin/bash
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
stage="$(mktemp -d "${TMPDIR:-/tmp}/native-window-observer.XXXXXX")"
trap 'rm -rf -- "$stage"' EXIT
# Exercise real producer/queue/consumer lifetime without a product dylib or
# private-symbol export policy. Unused GPU/compositor boundaries fail fast.
xcrun clang++ -std=c++20 -Wall -Wextra -Werror -fsanitize=address,undefined \
  -Wno-nullability-completeness -I"$root" -I"$root/compat" \
  -idirafter "${ANDROID_NDK_HOME:-$HOME/Library/Android/sdk/ndk/28.2.13676358}/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include" \
  -I"$root/_build/nativehelper-foundation/source/libnativehelper/include_jni" \
  -I"$root/_aosp/android16-surfaceflinger-core/buffer-native/libs/nativewindow/include" \
  -I"$root/_aosp/android16-surfaceflinger-core/buffer-native/libs/nativebase/include" \
  -I"$root/_aosp/frameworks/native/libs/nativebase/include" \
  -I"$root/_aosp/system/core/libcutils/include" \
  -I"$root/_aosp/system/core/libsystem/include" \
  -I"$root/_aosp/frameworks/native/libs/arect/include" \
  "$root/compat/darwin_android_native_window.cc" \
  "$root/compat/window/native_window_buffer_queue.cc" \
  "$root/compat/window/native_window_software_queue.cc" \
  "$root/compat/window/native_window_transaction_consumer.cc" \
  "$root/compat/window/locked_surface.cc" \
  "$root/compat/media/consumer_buffer.cc" \
  "$root/tools/tests/native-window-observer-boundaries.cc" \
  "$root/tools/tests/native-window-observer-test.cc" \
  -o "$stage/test"
"$stage/test"
