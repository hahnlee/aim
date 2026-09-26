#!/bin/bash
# ImageReader over the real native-window producer and IOSurface-backed
# hardware buffers, linked as a component (no product dylib, no private
# exports). Compositor and GPU boundaries fail fast if reached.
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
ndk="${ANDROID_NDK_HOME:-$HOME/Library/Android/sdk/ndk/28.2.13676358}"
stage="$(mktemp -d "${TMPDIR:-/tmp}/image-reader-ndk-runtime.XXXXXX")"
trap 'rm -rf -- "$stage"' EXIT
# The hardware buffer owner's handle transport uses deprecated global IOSurface
# IDs (see hardware-buffer-owner.sh).
xcrun clang++ -std=c++20 -Wall -Wextra -Werror -Wno-nullability-completeness \
  -Wno-error=deprecated-declarations -fsanitize=address,undefined -pthread \
  -DDARWIN_ART_FIXTURE_REAL_HARDWARE_BUFFERS \
  -I"$root" -I"$root/compat" -I"$root/tools/bionic-socket-broker-adapter/include" \
  -idirafter "$ndk/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include" \
  -I"$root/_build/nativehelper-foundation/source/libnativehelper/include_jni" \
  -I"$root/_aosp/android16-surfaceflinger-core/buffer-native/libs/nativewindow/include" \
  -I"$root/_aosp/android16-surfaceflinger-core/buffer-native/libs/nativebase/include" \
  -I"$root/_aosp/frameworks/native/libs/nativebase/include" \
  -I"$root/_aosp/frameworks/native/libs/nativewindow/include" \
  -I"$root/_aosp/frameworks/native/include" \
  -I"$root/_aosp/system/core/libcutils/include" \
  -I"$root/_aosp/system/core/libsystem/include" \
  -I"$root/_aosp/frameworks/native/libs/arect/include" \
  "$root/compat/media/image_reader_ndk.cc" \
  "$root/compat/media/image_consumer_queue.cc" \
  "$root/compat/media/consumer_buffer.cc" \
  "$root/compat/darwin_android_native_window.cc" \
  "$root/compat/window/native_window_buffer_queue.cc" \
  "$root/compat/window/native_window_software_queue.cc" \
  "$root/compat/window/native_window_transaction_consumer.cc" \
  "$root/compat/window/locked_surface.cc" \
  "$root/compat/graphics/hardware_buffer_owner.mm" \
  "$root/tools/tests/native-window-observer-boundaries.cc" \
  "$root/tools/tests/image-reader-ndk-runtime-test.cc" \
  -framework Foundation -framework IOSurface -framework Metal \
  -o "$stage/test"
ASAN_OPTIONS=detect_leaks=0:halt_on_error=1 UBSAN_OPTIONS=halt_on_error=1 "$stage/test"
