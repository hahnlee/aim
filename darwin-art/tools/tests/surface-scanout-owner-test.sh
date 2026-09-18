#!/bin/zsh
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
sdk="$(xcrun --sdk macosx --show-sdk-path)"
tmp="$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-surface-scanout-owner.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT

  "$(xcrun --find clang++)" -arch arm64 -isysroot "$sdk" -std=c++20 \
  -ObjC++ -fobjc-arc -O1 -g -Wall -Wextra -Werror \
  -fsanitize=address,undefined -fno-omit-frame-pointer -pthread \
  -I"$root" -I"$root/compat" \
  -I"$root/tools/bionic-socket-broker-adapter/include" \
  "$root/compat/window/composition_fence_monitor.cc" \
  "$root/compat/window/surface_scanout_owner.mm" \
  "$root/tools/tests/surface-scanout-owner-test.cc" \
  -o "$tmp/surface-scanout-owner-test"

"$tmp/surface-scanout-owner-test"
