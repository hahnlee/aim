#!/bin/bash
set -euo pipefail

project_root="$(cd "$(dirname "$0")/.." && pwd)"
bridge_root="$project_root/tools/android-register-natives-bridge"
stage="$(mktemp -d "${TMPDIR:-/tmp}/registered-native-cache.XXXXXX")"
trap 'rm -rf "$stage"' EXIT
cxx="$(xcrun --find clang++)"
sdk="$(xcrun --sdk macosx --show-sdk-path)"
flags=(-std=c++20 -isysroot "$sdk" -O1 -g -Wall -Wextra -Werror -fsanitize=address,undefined
       -I"$bridge_root/include")

# Component tests only: these do not load ART or establish APK acceptance.
"$cxx" "${flags[@]}" "$bridge_root/registered_native_bridge.cc" \
  "$bridge_root/registered_native_bridge_smoke.cc" -o "$stage/smoke"
"$stage/smoke" 0x10000000 0x10010000 0x10001000 0x10002000
"$cxx" "${flags[@]}" "$bridge_root/registered_native_bridge.cc" \
  "$bridge_root/registered_native_lifetime_test.cc" -o "$stage/lifetime"
"$stage/lifetime"
