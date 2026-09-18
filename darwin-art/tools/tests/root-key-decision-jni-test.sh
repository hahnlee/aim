#!/usr/bin/env bash
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
stage=$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-key-decision-jni.XXXXXX")
trap 'rm -rf "$stage"' EXIT
jdk=/opt/homebrew/opt/openjdk@17/libexec/openjdk.jdk/Contents/Home
clang++ -std=c++20 -fobjc-arc -Wall -Wextra -Werror -O1 -g \
  -fsanitize=address,undefined -fno-omit-frame-pointer -pthread \
  -I"$root" -I"$root/compat" -I"$jdk/include" -I"$jdk/include/darwin" \
  "$root/runtime/framework/wm/root_key_decision_jni.cc" \
  "$root/runtime/framework/input/root_key_authority.cc" \
  "$root/tools/tests/root-key-progress-unavailable.cc" \
  "$root/runtime/framework/input/input_routing_domain.cc" \
  "$root/runtime/framework/input/channel_resources.cc" \
  "$root/compat/window/desktop_root_events.mm" \
  "$root/compat/binder/wire_channel_lifetime.cc" \
  "$root/tools/tests/root-key-decision-jni-test.mm" \
  -framework AppKit -o "$stage/test"
ASAN_OPTIONS=detect_leaks=0:halt_on_error=1 UBSAN_OPTIONS=halt_on_error=1 "$stage/test"
