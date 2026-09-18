#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
ndk="${ANDROID_NDK_HOME:-$HOME/Library/Android/sdk/ndk/28.2.13676358}"
jni_include="$ndk/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include"
[[ -f "$jni_include/jni.h" ]] || {
  echo "media-codec-surface-lifetime: pinned JNI include is missing: $jni_include" >&2
  exit 2
}

stage="$(mktemp -d "${TMPDIR:-/tmp}/media-codec-surface-lifetime.XXXXXX")"
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
  -Wno-unused-function
  -Wno-nullability-completeness
  "-fsanitize=$sanitizers"
  -fno-omit-frame-pointer
  -I"$root/compat"
  -I/opt/homebrew/include
  -idirafter "$jni_include"
  -pthread
)

# Compile the actual production MediaCodec owner; only JNI and narrow window
# ports are stubbed by the test. No fixture verifier or copied codec state model
# is linked.
"$cxx" "${flags[@]}" -c "$root/compat/darwin_media_codec.cc" \
  -o "$stage/darwin_media_codec.o"
"$cxx" "${flags[@]}" -c "$root/tools/tests/media-codec-surface-lifetime-test.cc" \
  -o "$stage/test.o"
"$cxx" "${flags[@]}" "$stage/test.o" "$stage/darwin_media_codec.o" \
  -framework CoreFoundation \
  -framework CoreMedia -framework CoreVideo -framework VideoToolbox \
  -o "$stage/media-codec-surface-lifetime-test"
ASAN_OPTIONS=detect_leaks=0:halt_on_error=1 \
UBSAN_OPTIONS=halt_on_error=1 \
  "$stage/media-codec-surface-lifetime-test"
