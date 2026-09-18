#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
stage="$(mktemp -d "${TMPDIR:-/tmp}/surface-control-state.XXXXXX")"
trap 'rm -rf -- "$stage"' EXIT
ndk="${ANDROID_NDK_ROOT:-/Users/hahnlee/Library/Android/sdk/ndk/28.2.13676358}/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include"
sdk="$(xcrun --sdk macosx --show-sdk-path)"
xcrun clang++ -arch arm64 -isysroot "$sdk" -std=c++20 -Wall -Wextra -Werror \
  -I"$root" -I"$root/include" -I"$root/compat" -idirafter "$ndk" \
  -idirafter "$ndk/aarch64-linux-android" \
  "$root/compat/window/surface_control_state.cc" \
  "$root/tools/tests/surface-control-state-test.cc" \
  -o "$stage/surface-control-state-test"
"$stage/surface-control-state-test"
