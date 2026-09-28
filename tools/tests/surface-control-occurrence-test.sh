#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
ndk="${ANDROID_NDK_HOME:-$HOME/Library/Android/sdk/ndk/28.2.13676358}"
include="$ndk/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include"
stage="$(mktemp -d "${TMPDIR:-/tmp}/surface-control-occurrence.XXXXXX")"
trap 'rm -rf -- "$stage"' EXIT
sdk="$(xcrun --sdk macosx --show-sdk-path)"
xcrun clang++ -arch arm64 -isysroot "$sdk" -std=c++20 -O1 -g \
  -Wall -Wextra -Werror -Wno-nullability-completeness \
  -fsanitize=address,undefined -fno-omit-frame-pointer \
  -I"$root" -I"$root/include" -I"$root/compat" -idirafter "$include" \
  "$root/compat/window/surface_control_state.cc" \
  "$root/compat/window/surface_control_registry.cc" \
  "$root/compat/window/surface_transaction_lifetime.cc" \
  "$root/compat/window/surface_transaction_builder.cc" \
  "$root/tools/tests/surface-control-occurrence-test.cc" -o "$stage/test"
ASAN_OPTIONS=detect_leaks=0:halt_on_error=1 UBSAN_OPTIONS=halt_on_error=1 \
  "$stage/test"
