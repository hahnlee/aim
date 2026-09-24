#!/bin/bash
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
stage=$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-root-geometry.XXXXXX")
trap 'rm -rf "$stage"' EXIT
sdk=$(xcrun --sdk macosx --show-sdk-path)
"${CXX:-clang++}" -arch arm64 -isysroot "$sdk" -std=c++20 -O1 -g -UNDEBUG \
  -Wall -Wextra -Werror -pthread -I"$root" \
  "$root/compat/window/root_geometry.cc" \
  "$root/tools/tests/root-geometry-test.cc" -o "$stage/test"
"$stage/test"
