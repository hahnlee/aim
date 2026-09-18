#!/bin/bash
set -euo pipefail

root=$(cd "$(dirname "$0")/../.." && pwd)
stage=$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-desktop-root-deferred-fact.XXXXXX")
trap 'rm -rf "$stage"' EXIT
cxx=${CXX:-clang++}
sdk=$(xcrun --sdk macosx --show-sdk-path)

"$cxx" -arch arm64 -isysroot "$sdk" -std=c++20 -ObjC++ -fobjc-arc -O1 -g \
  -Wall -Wextra -Werror -fsanitize=address,undefined \
  -fno-omit-frame-pointer -pthread -I"$root" \
  "$root/compat/window/desktop_root_events.mm" \
  "$root/tools/tests/desktop-root-deferred-fact-test.mm" \
  -framework AppKit -o "$stage/desktop-root-deferred-fact-test"
"$stage/desktop-root-deferred-fact-test"
