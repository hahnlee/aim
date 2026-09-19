#!/bin/bash
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
ndk="${ANDROID_NDK_HOME:-$HOME/Library/Android/sdk/ndk/28.2.13676358}"
jni_include="$ndk/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include"
[[ -f "$jni_include/jni.h" ]] || {
  echo "texture-view-jni: pinned JNI include is missing: $jni_include" >&2
  exit 2
}

stage="$(mktemp -d "${TMPDIR:-/tmp}/texture-view-jni.XXXXXX")"
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
  -I"$root/compat"
  -I"$root/_aosp/frameworks/base/libs/hwui/apex/include"
  -I"$root/_aosp/system/core/libcutils/include"
  -idirafter "$jni_include"
)

"$cxx" "${flags[@]}" -c "$root/compat/window/texture_view_jni.cc" \
  -o "$stage/texture_view_jni.o"
"$cxx" "${flags[@]}" -c "$root/tools/tests/texture-view-jni-test.cc" \
  -o "$stage/test.o"
"$cxx" "${flags[@]}" "$stage/test.o" "$stage/texture_view_jni.o" \
  -o "$stage/texture-view-jni-test"

ASAN_OPTIONS=detect_leaks=0:halt_on_error=1 \
UBSAN_OPTIONS=halt_on_error=1 \
  "$stage/texture-view-jni-test"
