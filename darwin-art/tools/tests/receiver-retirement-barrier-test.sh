#!/bin/sh
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
out=$(mktemp "${TMPDIR:-/tmp}/darwin-art-receiver-barrier.XXXXXX")
trap 'rm -f "$out"' EXIT
sdk=$(xcrun --sdk macosx --show-sdk-path)
"${CXX:-clang++}" -arch arm64 -isysroot "$sdk" -std=c++20 -ObjC++ -fobjc-arc -O1 -g -Wall -Wextra -Werror \
  -fsanitize=address,undefined -fno-omit-frame-pointer \
  -I"$root" -I"$root/compat" -I"$root/_aosp/frameworks/native/include" \
  "$root/compat/looper/reusable_task.cc" \
  "$root/tools/tests/input-owner-task-fixture.cc" \
  "$root/runtime/framework/input/claimed_input_transport_pump.cc" \
  "$root/tools/tests/receiver-retirement-barrier-test.cc" \
  "$root/runtime/framework/input/receiver_retirement_barrier.cc" \
  "$root/runtime/framework/input/pending_receiver_retirement.cc" \
  "$root/runtime/framework/input/receiver_retirement_driver.cc" \
  "$root/runtime/framework/input/receiver_routing_lifecycle.cc" \
  "$root/runtime/framework/input/receiver_admission.cc" \
  "$root/runtime/framework/input/receiver_endpoint_binding.cc" \
  "$root/runtime/framework/input/input_transport_pump.cc" \
  "$root/runtime/framework/input/input_transport_readiness.cc" \
  "$root/runtime/framework/input/input_transport.cc" \
  "$root/runtime/framework/input/input_framed_reader.cc" \
  "$root/runtime/framework/input/input_resource_progress.cc" \
  "$root/runtime/framework/input/transport_registration_authority.cc" \
  "$root/runtime/framework/input/input_routing_domain.cc" \
  "$root/runtime/framework/input/input_routing.cc" \
  "$root/runtime/framework/input/input_routing_actions.cc" \
  "$root/runtime/framework/input/root_key_routing.cc" \
  "$root/runtime/framework/input/root_key_authority.cc" \
  "$root/compat/window/desktop_root_events.mm" \
  "$root/compat/binder/wire_channel_lifetime.cc" \
  "$root/runtime/framework/input/input_routing_focus.cc" \
  "$root/runtime/framework/input/routing_transport_dispatch.cc" \
  -framework AppKit -o "$out"
"$out"
