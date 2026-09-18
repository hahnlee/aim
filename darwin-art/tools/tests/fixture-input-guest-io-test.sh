#!/bin/sh
set -eu

repo_dir=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
build_dir=$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-fixture-input-guest-io.XXXXXX")
trap 'rm -rf "$build_dir"' EXIT HUP INT TERM
cxx=${CXX:-c++}
ndk_root=${ANDROID_NDK_HOME:-$HOME/Library/Android/sdk/ndk/28.2.13676358}
ndk_include="$ndk_root/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include"
[ -f "$ndk_include/jni.h" ] || exit 2

"$cxx" -std=c++20 -Wall -Wextra -Werror \
  -I"$repo_dir" -I"$repo_dir/compat" -idirafter "$ndk_include" \
  "$repo_dir/tools/tests/fixture-input-guest-io-test.cc" \
  "$repo_dir/probes/fixture_input_guest_io.cc" \
  -o "$build_dir/fixture-input-guest-io-test"
"$build_dir/fixture-input-guest-io-test"
echo "fixture-input-guest-io: PASS"
