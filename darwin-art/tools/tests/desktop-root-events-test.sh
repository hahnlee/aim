#!/bin/bash
set -euo pipefail

root=$(cd "$(dirname "$0")/../.." && pwd)
stage=$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-desktop-root-events.XXXXXX")
trap 'rm -rf "$stage"' EXIT
cxx=${CXX:-clang++}
sdk=$(xcrun --sdk macosx --show-sdk-path)
clang -std=c11 -Wall -Wextra -Werror -fsyntax-only -I"$root" \
  "$root/tools/tests/desktop-root-event-abi-test.c"
# Public root headers are consumed by ordinary C++ input owners as well as
# Objective-C++ host adapters; ObjC weak storage must stay implementation-only.
"$cxx" -x c++ -std=c++20 -Wall -Wextra -Werror -fsyntax-only -I"$root" \
  "$root/tools/tests/desktop-root-event-abi-test.c"

"$cxx" -arch arm64 -isysroot "$sdk" -std=c++20 -ObjC++ -fobjc-arc -O1 -g \
  -Wall -Wextra -Werror -pthread -I"$root" \
  "$root/compat/window/desktop_root_events.mm" \
  "$root/tools/tests/desktop-root-events-test.mm" \
  -framework AppKit -o "$stage/desktop-root-events-test"
"$stage/desktop-root-events-test"
