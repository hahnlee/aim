#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "$0")/../.." && pwd)
stage=$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-root-key-authority.XXXXXX")
trap 'rm -rf "$stage"' EXIT
sdk=$(xcrun --sdk macosx --show-sdk-path)
cxx=${CXX:-clang++}

"$cxx" -arch arm64 -isysroot "$sdk" -std=c++20 -ObjC++ -fobjc-arc -O1 -g \
  -Wall -Wextra -Werror -fsanitize=address,undefined -fno-omit-frame-pointer \
  -pthread -I"$root" -I"$root/compat" \
  "$root/compat/window/desktop_root_events.mm" \
  "$root/compat/binder/wire_channel_lifetime.cc" \
  "$root/runtime/framework/input/input_routing_domain.cc" \
  "$root/runtime/framework/input/root_key_authority.cc" \
  "$root/tools/tests/root-key-progress-unavailable.cc" \
  "$root/tools/tests/root-key-authority-test.mm" \
  -framework AppKit -o "$stage/root-key-authority-test"

ASAN_OPTIONS=detect_leaks=0:halt_on_error=1 \
UBSAN_OPTIONS=halt_on_error=1 \
  "$stage/root-key-authority-test"
