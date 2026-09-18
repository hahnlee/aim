#!/bin/bash
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
stage=$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-root-target.XXXXXX")
trap 'rm -rf "$stage"' EXIT
sdk=$(xcrun --sdk macosx --show-sdk-path)
"${CXX:-clang++}" -arch arm64 -isysroot "$sdk" -std=c++20 -fobjc-arc -O1 -g \
  -Wall -Wextra -Werror -pthread -I"$root" \
  "$root/compat/window/desktop_root_events.mm" \
  "$root/compat/window/desktop_root_target.mm" \
  "$root/tools/tests/desktop-root-target-test.mm" \
  -framework AppKit -framework CoreFoundation -o "$stage/test"
"$stage/test"
