#!/bin/zsh
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
cxx="$(xcrun --find clang++)"
sdk="$(xcrun --sdk macosx --show-sdk-path)"
tmp="$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-routing-scheduler.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT
"$cxx" -arch arm64 -isysroot "$sdk" -std=c++20 -ObjC++ -fobjc-arc -Wall -Wextra -Werror -pthread \
  -fsanitize=address,undefined -fno-omit-frame-pointer \
  -I"$root" -I"$root/compat" \
  -I"$root/tools/bionic-socket-broker-adapter/include" \
  -I"$root/tools/bionic-errno-tls/include" \
  -I"$root/_aosp/frameworks/native/include" \
  "$root/compat/looper/android_looper_owner.cc" \
  "$root/compat/looper/reusable_task.cc" \
  "$root/runtime/framework/input/transport_registration_authority.cc" \
  "$root/runtime/framework/input/input_routing_domain.cc" \
  "$root/runtime/framework/input/input_routing.cc" \
  "$root/runtime/framework/input/input_routing_actions.cc" \
  "$root/runtime/framework/input/root_key_routing.cc" \
  "$root/runtime/framework/input/root_key_authority.cc" \
  "$root/compat/window/desktop_root_events.mm" \
  "$root/compat/binder/wire_channel_lifetime.cc" \
  "$root/runtime/framework/input/input_routing_focus.cc" \
  "$root/runtime/framework/input/input_transport.cc" \
  "$root/runtime/framework/input/input_framed_reader.cc" \
  "$root/runtime/framework/input/input_resource_progress.cc" \
  "$root/runtime/framework/input/input_transport_pump.cc" \
  "$root/runtime/framework/input/input_transport_readiness.cc" \
  "$root/runtime/framework/input/routing_transport_dispatch.cc" \
  "$root/runtime/framework/input/channel_endpoint.cc" \
  "$root/runtime/framework/input/finish_ledger.cc" \
  "$root/runtime/framework/input/channel_routing_continuation.cc" \
  "$root/runtime/framework/input/routing_transport_scheduler.cc" \
  "$root/tools/tests/routing-transport-scheduler-test.cc" \
  -framework AppKit -o "$tmp/routing-transport-scheduler-test"
"$tmp/routing-transport-scheduler-test"
