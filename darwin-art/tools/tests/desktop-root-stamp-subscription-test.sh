#!/usr/bin/env bash
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
stage=$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-root-stamp.XXXXXX")
trap 'rm -rf "$stage"' EXIT
sdk=$(xcrun --sdk macosx --show-sdk-path)
"${CXX:-clang++}" -arch arm64 -isysroot "$sdk" -std=c++20 -fobjc-arc -O1 -g \
  -Wall -Wextra -Werror -fsanitize=address,undefined -fno-omit-frame-pointer \
  -pthread -I"$root" "$root/compat/window/desktop_root_events.mm" \
  "$root/tools/tests/desktop-root-stamp-subscription-test.mm" \
  -framework AppKit -o "$stage/test"
for iteration in 1 2 3; do
  ASAN_OPTIONS=detect_leaks=0:halt_on_error=1 UBSAN_OPTIONS=halt_on_error=1 "$stage/test"
done
