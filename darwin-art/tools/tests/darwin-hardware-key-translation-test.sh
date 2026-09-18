#!/bin/bash
set -euo pipefail

root=$(cd "$(dirname "$0")/../.." && pwd)
stage=$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-key-translation.XXXXXX")
trap 'rm -rf "$stage"' EXIT
cxx=${CXX:-clang++}
sdk=$(xcrun --sdk macosx --show-sdk-path)

"$cxx" -arch arm64 -isysroot "$sdk" -std=c++20 -ObjC++ -O1 -g \
  -Wall -Wextra -Werror -I"$root" \
  "$root/compat/input/darwin_hardware_key_translation.mm" \
  "$root/tools/tests/darwin-hardware-key-translation-test.mm" \
  -framework AppKit -o "$stage/darwin-hardware-key-translation-test"
"$stage/darwin-hardware-key-translation-test"
