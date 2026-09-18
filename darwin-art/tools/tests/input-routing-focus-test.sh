#!/bin/sh
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
out=$(mktemp "${TMPDIR:-/tmp}/darwin-art-input-routing-focus.XXXXXX")
trap 'rm -f "$out"' EXIT
sdk=$(xcrun --sdk macosx --show-sdk-path)
clang++ -arch arm64 -isysroot "$sdk" -std=c++20 -ObjC++ -fobjc-arc -O1 -g -Wall -Wextra -Werror \
  -fsanitize=address,undefined -fno-omit-frame-pointer \
  -I"$root" -I"$root/compat" \
  "$root/tools/tests/input-routing-focus-test.cc" \
  "$root/runtime/framework/input/input_routing_domain.cc" \
  "$root/runtime/framework/input/input_routing.cc" \
  "$root/runtime/framework/input/input_routing_actions.cc" \
  "$root/runtime/framework/input/input_routing_focus.cc" \
  "$root/runtime/framework/input/root_key_routing.cc" \
  "$root/runtime/framework/input/root_key_authority.cc" \
  "$root/tools/tests/root-key-progress-unavailable.cc" \
  "$root/compat/window/desktop_root_events.mm" \
  "$root/compat/binder/wire_channel_lifetime.cc" \
  -framework AppKit -o "$out"
"$out"
