#!/bin/bash
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
ndk="${ANDROID_NDK_HOME:-$HOME/Library/Android/sdk/ndk/28.2.13676358}"
sanitizer="${SANITIZER:-address,undefined}"
stage="$(mktemp -d "${TMPDIR:-/tmp}/image-reader-ndk.XXXXXX")"
trap 'rm -rf -- "$stage"' EXIT
xcrun clang++ -std=c++20 -Wall -Wextra -Werror -Wno-nullability-completeness \
  -fsanitize="$sanitizer" -pthread \
  -I"$root" \
  -I"$root/_aosp/frameworks/native/libs/nativewindow/include" \
  -I"$root/_aosp/frameworks/native/include" \
  -I"$root/_aosp/frameworks/native/libs/arect/include" \
  -I"$root/_aosp/system/core/libsystem/include" \
  -idirafter "$ndk/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include" \
  "$root/tools/tests/image-reader-ndk-test.cc" \
  "$root/compat/media/image_reader_ndk.cc" \
  "$root/compat/media/image_consumer_queue.cc" \
  "$root/compat/media/consumer_buffer.cc" \
  -o "$stage/test"
"$stage/test"
