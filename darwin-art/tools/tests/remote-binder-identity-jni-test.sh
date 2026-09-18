#!/bin/bash
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
stage="$(mktemp -d "${TMPDIR:-/tmp}/remote-binder-identity-jni.XXXXXX")"
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
  -fsanitize="$sanitizers"
  -fno-omit-frame-pointer
  -I"$root/compat"
  -idirafter "$root/_aosp/libnativehelper-full/include_jni"
)

"$cxx" "${flags[@]}" -c "$root/compat/binder/remote_binder_identity_jni.cc" \
  -o "$stage/remote_binder_identity_jni.o"
"$cxx" "${flags[@]}" -c "$root/tools/tests/remote-binder-identity-jni-test.cc" \
  -o "$stage/test.o"
"$cxx" "${flags[@]}" "$stage/test.o" "$stage/remote_binder_identity_jni.o" \
  -o "$stage/remote-binder-identity-jni-test"

ASAN_OPTIONS=detect_leaks=0:halt_on_error=1 \
UBSAN_OPTIONS=halt_on_error=1 \
  "$stage/remote-binder-identity-jni-test"
