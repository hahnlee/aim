#!/bin/sh
set -eu

repo_dir=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
ndk_root=${ANDROID_NDK_ROOT:-/Users/hahnlee/Library/Android/sdk/ndk/28.2.13676358}
sysroot="$ndk_root/toolchains/llvm/prebuilt/darwin-x86_64/sysroot"
sdk=$(xcrun --sdk macosx --show-sdk-path)
build_dir=$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-prepared-surface-commit.XXXXXX")
trap 'rm -rf "$build_dir"' EXIT

xcrun clang++ -arch arm64 -isysroot "$sdk" -std=c++20 -Wall -Wextra -Werror \
  -I"$repo_dir" -I"$repo_dir/include" -I"$repo_dir/compat" \
  -idirafter "$sysroot/usr/include" \
  -idirafter "$sysroot/usr/include/aarch64-linux-android" \
  "$repo_dir/compat/window/surface_control_state.cc" \
  "$repo_dir/compat/window/surface_control_registry.cc" \
  "$repo_dir/tools/tests/prepared-surface-commit-test.cc" \
  -o "$build_dir/prepared-surface-commit-test"

"$build_dir/prepared-surface-commit-test"
