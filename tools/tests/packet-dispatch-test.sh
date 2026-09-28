#!/bin/bash
set -euo pipefail
test_root="$(cd "$(dirname "$0")/../.." && pwd)"
ndk_root="${ANDROID_NDK_HOME:-$HOME/Library/Android/sdk/ndk/28.2.13676358}"
ndk_include="$ndk_root/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include"
[[ -f "$ndk_include/jni.h" ]] || exit 2
stage="$(mktemp -d "${TMPDIR:-/tmp}/packet-dispatch.XXXXXX")"
trap 'rm -rf -- "$stage"' EXIT
sdk_path="$(xcrun --sdk macosx --show-sdk-path)"
cxx_path="$(xcrun --find clang++)"
flags=(
  -arch arm64 -isysroot "$sdk_path" -std=c++20 -O1 -g
  -Wall -Wextra -Werror -Wno-nullability-completeness -pthread
  -I"$test_root" -I"$test_root/compat" -idirafter "$ndk_include"
)
"$cxx_path" "${flags[@]}" -c "$test_root/runtime/framework/input/packet_dispatch.cc" \
  -o "$stage/packet-dispatch.o"
"$cxx_path" "${flags[@]}" -c "$test_root/tools/tests/packet-dispatch-test.cc" \
  -o "$stage/test.o"
"$cxx_path" "${flags[@]}" "$stage/packet-dispatch.o" "$stage/test.o" \
  -o "$stage/packet-dispatch-test"
"$stage/packet-dispatch-test"
