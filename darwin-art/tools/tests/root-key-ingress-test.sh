#!/usr/bin/env bash
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
stage=$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-root-key-ingress.XXXXXX")
trap 'rm -rf "$stage"' EXIT HUP INT TERM
source_name=${1:-root-key-ingress-test.mm}
case "$source_name" in
  root-key-ingress-test.mm|root-key-ingress-lifetime-test.mm) ;;
  *) echo 'unknown isolated ingress test source' >&2; exit 64 ;;
esac

# The looper-posix-wake-ports object is a controlled host-pipe broker. It
# exercises the production ALooper/reusable-task code, but is not physical
# Android Binder/ALooper acceptance.
clang++ -std=c++20 -ObjC++ -fobjc-arc -O1 -g -Wall -Wextra -Werror -pthread \
  -fsanitize=address,undefined -fno-omit-frame-pointer \
  -I"$root" -I"$root/compat" \
  -I"$root/tools/bionic-socket-broker-adapter/include" \
  -I"$root/tools/bionic-errno-tls/include" \
  -I"$root/_aosp/frameworks/native/include" \
  "$root/compat/looper/android_looper_owner.cc" \
  "$root/compat/looper/reusable_task.cc" \
  "$root/tools/tests/looper-posix-wake-ports.cc" \
  "$root/compat/window/desktop_root_events.mm" \
  "$root/compat/binder/wire_channel_lifetime.cc" \
  "$root/runtime/framework/input/root_key_authority.cc" \
  "$root/runtime/framework/input/root_key_ingress.cc" \
  "$root/runtime/framework/input/root_key_ingress_registry.cc" \
  "$root/runtime/framework/input/root_key_routing.cc" \
  "$root/runtime/framework/input/input_routing_domain.cc" \
  "$root/runtime/framework/input/input_routing.cc" \
  "$root/runtime/framework/input/input_routing_actions.cc" \
  "$root/runtime/framework/input/input_routing_focus.cc" \
  "$root/runtime/framework/input/input_routing_packet_lease.cc" \
  "$root/tools/tests/$source_name" \
  -framework AppKit -o "$stage/root-key-ingress-test"

ASAN_OPTIONS=detect_leaks=0:halt_on_error=1 \
UBSAN_OPTIONS=halt_on_error=1 \
  "$stage/root-key-ingress-test"
