#!/bin/bash
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
ndk="${ANDROID_NDK_HOME:-$HOME/Library/Android/sdk/ndk/28.2.13676358}"
ndk_include="$ndk/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include"
[[ -f "$ndk_include/android/surface_control.h" ]] || {
  echo "blast-jni: pinned NDK include is missing: $ndk_include" >&2
  exit 2
}

stage="$(mktemp -d "${TMPDIR:-/tmp}/blast-jni.XXXXXX")"
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
  -ffunction-sections
  -fdata-sections
  -I"$root/compat"
  -idirafter "$ndk_include"
)

# Link the real BLAST JNI adapter and transaction state objects. Platform,
# JNI, native-window, and transaction APIs are narrowly faked by the test;
# no product/probe/SF archive is linked.
"$cxx" "${flags[@]}" -c "$root/compat/window/blast_buffer_queue_jni.cc" \
  -o "$stage/blast_buffer_queue_jni.o"
"$cxx" "${flags[@]}" -c "$root/compat/window/blast_transaction_state.cc" \
  -o "$stage/blast_transaction_state.o"
"$cxx" "${flags[@]}" -c "$root/tools/tests/blast-jni-test.cc" \
  -o "$stage/test.o"
"$cxx" "${flags[@]}" -Wl,-dead_strip \
  "$stage/test.o" "$stage/blast_buffer_queue_jni.o" \
  "$stage/blast_transaction_state.o" -o "$stage/blast-jni-test"

ASAN_OPTIONS=detect_leaks=0:halt_on_error=1 \
UBSAN_OPTIONS=halt_on_error=1 \
  "$stage/blast-jni-test"
