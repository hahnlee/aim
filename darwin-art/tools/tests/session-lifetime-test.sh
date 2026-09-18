#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
ndk="${ANDROID_NDK_HOME:-$HOME/Library/Android/sdk/ndk/28.2.13676358}"
jni_include="$ndk/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include"
[[ -f "$jni_include/jni.h" ]] || {
  echo "session-lifetime: pinned JNI include is missing: $jni_include" >&2
  exit 2
}

stage="$(mktemp -d "${TMPDIR:-/tmp}/session-lifetime.XXXXXX")"
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
  -pthread
  "-fsanitize=$sanitizers"
  -fno-omit-frame-pointer
  -I"$root"
  -I"$root/include"
  -idirafter "$jni_include"
)

# Compile the actual production lifetime owner and its real timestamp state.
# No graphics-session wrapper, product archive, probe source, or platform
# implementation is linked; the test drives the owner-level admission API.
"$cxx" "${flags[@]}" \
  "$root/runtime/embedding/session_lifetime.cc" \
  "$root/runtime/embedding/graphics_state.cc" \
  "$root/tools/tests/session-lifetime-test.cc" \
  -o "$stage/session-lifetime-test"

ASAN_OPTIONS=detect_leaks=0:halt_on_error=1 \
UBSAN_OPTIONS=halt_on_error=1 \
  "$stage/session-lifetime-test"
