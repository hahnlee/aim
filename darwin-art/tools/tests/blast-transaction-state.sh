#!/bin/bash
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
ndk="${ANDROID_NDK_HOME:-$HOME/Library/Android/sdk/ndk/28.2.13676358}"
ndk_include="$ndk/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include"
[[ -f "$ndk_include/android/surface_control.h" ]] || {
  echo "blast-transaction-state: pinned NDK include is missing: $ndk_include" >&2
  exit 2
}

stage="$(mktemp -d "${TMPDIR:-/tmp}/blast-transaction-state.XXXXXX")"
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
  -idirafter "$ndk_include"
)

# Link the real production state TU only; no JNI, product, probe, or
# SurfaceFlinger archive is part of this ownership test.
"$cxx" "${flags[@]}" -c "$root/compat/window/blast_transaction_state.cc" \
  -o "$stage/blast_transaction_state.o"
"$cxx" "${flags[@]}" -c "$root/tools/tests/blast-transaction-state-test.cc" \
  -o "$stage/test.o"
"$cxx" "${flags[@]}" "$stage/test.o" "$stage/blast_transaction_state.o" \
  -o "$stage/blast-transaction-state-test"

ASAN_OPTIONS=detect_leaks=0:halt_on_error=1 \
UBSAN_OPTIONS=halt_on_error=1 \
  "$stage/blast-transaction-state-test"
