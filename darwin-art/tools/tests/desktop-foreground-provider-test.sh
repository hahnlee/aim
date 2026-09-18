#!/bin/bash
set -euo pipefail

root=$(cd "$(dirname "$0")/../.." && pwd)
stage=$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-desktop-foreground.XXXXXX")
trap 'rm -rf "$stage"' EXIT

sdk=$(xcrun --sdk macosx --show-sdk-path)
"${CXX:-clang++}" -arch arm64 -isysroot "$sdk" -std=c++20 -O1 -g \
  -Wall -Wextra -Werror -pthread -I"$root" \
  "$root/compat/window/desktop_foreground_provider.cc" \
  "$root/tools/tests/desktop-foreground-provider-test.cc" \
  -framework ApplicationServices -o "$stage/test"
"$stage/test"
