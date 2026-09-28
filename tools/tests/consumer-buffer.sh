#!/bin/bash
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
ndk="${ANDROID_NDK_HOME:-$HOME/Library/Android/sdk/ndk/28.2.13676358}"
stage="$(mktemp -d "${TMPDIR:-/tmp}/consumer-buffer.XXXXXX")"
trap 'rm -rf -- "$stage"' EXIT
xcrun clang++ -std=c++20 -Wall -Wextra -Werror -Wno-nullability-completeness \
  -fsanitize=address,undefined \
  -I"$root/_aosp/frameworks/native/libs/nativewindow/include" \
  -I"$root/_aosp/frameworks/native/libs/arect/include" \
  -I"$root/_aosp/system/core/libsystem/include" \
  -I"$root/_build/nativehelper-foundation/source/libnativehelper/include_jni" \
  -idirafter "$ndk/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include" \
  "$root/tools/tests/consumer-buffer-test.cc" \
  "$root/compat/media/consumer_buffer.cc" -o "$stage/test"
"$stage/test"
