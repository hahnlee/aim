#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "$0")/../.." && pwd)
stage=$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-native-binder-endpoint.XXXXXX")
trap 'rm -rf "$stage"' EXIT
jdk=/opt/homebrew/opt/openjdk@17/libexec/openjdk.jdk/Contents/Home
fixture="$root/tools/tests/native-binder-endpoint-lifetime-fixtures"

# The fixture headers shadow only the AOSP Binder/JNI boundary.  The provider
# translation unit is the real production object; this is controlled seam
# coverage, not a physical-device or genuine AOSP Binder acceptance test.
clang++ -std=c++20 -Wall -Wextra -Werror -O1 -g \
  -fsanitize=address,undefined -fno-omit-frame-pointer -pthread \
  -I"$fixture" -I"$root" -I"$root/include" -I"$root/compat" \
  -I"$jdk/include" -I"$jdk/include/darwin" \
  "$root/compat/binder/native_endpoint_lifetime.cc" \
  "$root/tools/tests/native-binder-endpoint-lifetime-test.cc" \
  -o "$stage/test"

ASAN_OPTIONS=detect_leaks=0:halt_on_error=1 UBSAN_OPTIONS=halt_on_error=1 \
  "$stage/test"
