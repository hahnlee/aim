#!/bin/bash
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
ndk="${ANDROID_NDK_HOME:-$HOME/Library/Android/sdk/ndk/28.2.13676358}"
jni_include="$ndk/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include"
[[ -f "$jni_include/jni.h" ]] || {
  echo "sync-fence-jni: pinned JNI include is missing: $jni_include" >&2
  exit 2
}

stage="$(mktemp -d "${TMPDIR:-/tmp}/sync-fence-jni.XXXXXX")"
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
  -idirafter "$jni_include"
)

# Compile the extracted production TU as a separate object. In particular,
# this test does not include the implementation text or link a product/probe
# archive, so registration and lifetime regressions stay local to the window
# owner.
"$cxx" "${flags[@]}" -c "$root/compat/window/sync_fence_jni.cc" \
  -o "$stage/sync_fence_jni.o"
"$cxx" "${flags[@]}" -c "$root/tools/tests/sync-fence-jni-test.cc" \
  -o "$stage/test.o"
"$cxx" "${flags[@]}" "$stage/test.o" "$stage/sync_fence_jni.o" \
  -o "$stage/sync-fence-jni-test"

ASAN_OPTIONS=detect_leaks=0:halt_on_error=1 \
UBSAN_OPTIONS=halt_on_error=1 \
  "$stage/sync-fence-jni-test"
