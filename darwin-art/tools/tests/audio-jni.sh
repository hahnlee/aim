#!/bin/bash
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
ndk="${ANDROID_NDK_HOME:-$HOME/Library/Android/sdk/ndk/28.2.13676358}"
jni_include="$ndk/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include"
[[ -f "$jni_include/jni.h" ]] || {
  echo "audio-jni: pinned JNI include is missing: $jni_include" >&2
  exit 2
}

stage="$(mktemp -d "${TMPDIR:-/tmp}/audio-jni.XXXXXX")"
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

# Keep both owners as independent production objects. This test links no
# product archive, probe object, or CoreAudio implementation.
"$cxx" "${flags[@]}" -c "$root/compat/media/audio_system_jni.cc" \
  -o "$stage/audio_system_jni.o"
"$cxx" "${flags[@]}" -c "$root/compat/media/audio_track_jni.cc" \
  -o "$stage/audio_track_jni.o"
"$cxx" "${flags[@]}" -c "$root/tools/tests/audio-jni-test.cc" \
  -o "$stage/test.o"
"$cxx" "${flags[@]}" "$stage/test.o" "$stage/audio_system_jni.o" \
  "$stage/audio_track_jni.o" -o "$stage/audio-jni-test"

ASAN_OPTIONS=detect_leaks=0:halt_on_error=1 \
UBSAN_OPTIONS=halt_on_error=1 \
  "$stage/audio-jni-test"
