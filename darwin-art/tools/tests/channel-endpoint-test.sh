#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
stage="$(mktemp -d "${TMPDIR:-/tmp}/channel-endpoint.XXXXXX")"
trap 'rm -rf -- "$stage"' EXIT
sdk="$(xcrun --sdk macosx --show-sdk-path)"
clang++ -arch arm64 -isysroot "$sdk" -std=c++20 -ObjC++ -fobjc-arc -O1 -g -Wall -Wextra -Werror \
  -fsanitize=address,undefined -fno-omit-frame-pointer \
  -I"$root" -I"$root/compat" -I"$root/tools/bionic-errno-tls/include" \
  -I"$root/_aosp/libnativehelper/include_jni" \
  -I"$root/_aosp/frameworks/native/include" \
  "$root/compat/looper/reusable_task.cc" \
  "$root/tools/tests/input-owner-task-fixture.cc" \
  "$root/runtime/framework/input/claimed_input_transport_pump.cc" \
  "$root/runtime/framework/input/channel_endpoint.cc" \
  "$root/runtime/framework/input/finish_ledger.cc" \
  "$root/runtime/framework/input/channel_resources.cc" \
  "$root/runtime/framework/input/channel_identity_catalog.cc" \
  "$root/runtime/framework/input/input_channel_jni.cc" \
  "$root/runtime/framework/wm/window_input_publisher_jni.cc" \
  "$root/runtime/framework/wm/window_input_endpoint_lease.cc" \
  "$root/runtime/framework/input/channel_routing_continuation.cc" \
  "$root/runtime/framework/input/routing_transport_scheduler.cc" \
  "$root/runtime/framework/input/transport_registration_authority.cc" \
  "$root/runtime/framework/input/input_transport.cc" \
  "$root/runtime/framework/input/input_framed_reader.cc" \
  "$root/runtime/framework/input/input_resource_progress.cc" \
  "$root/runtime/framework/input/input_transport_pump.cc" \
  "$root/runtime/framework/input/input_transport_readiness.cc" \
  "$root/runtime/framework/input/input_routing_domain.cc" \
  "$root/runtime/framework/input/input_routing.cc" \
  "$root/runtime/framework/input/input_routing_actions.cc" \
  "$root/runtime/framework/input/root_key_routing.cc" \
  "$root/runtime/framework/input/root_key_authority.cc" \
  "$root/compat/window/desktop_root_events.mm" \
  "$root/compat/binder/wire_channel_lifetime.cc" \
  "$root/runtime/framework/input/input_routing_focus.cc" \
  "$root/runtime/framework/input/routing_transport_dispatch.cc" \
  "$root/runtime/framework/input/receiver_registry.cc" \
  "$root/runtime/framework/input/receiver_admission.cc" \
  "$root/runtime/framework/input/receiver_jni_resources.cc" \
  "$root/runtime/framework/input/receiver_lifecycle.cc" \
  "$root/runtime/framework/input/receiver_transport_policy.cc" \
  "$root/runtime/framework/input/receiver_input_consumer.cc" \
  "$root/runtime/framework/input/input_routing_packet_lease.cc" \
  "$root/runtime/framework/input/receiver_focus_control.cc" \
  "$root/runtime/framework/input/receiver_focus_jni.cc" \
  "$root/runtime/framework/input/receiver_retirement_driver.cc" \
  "$root/runtime/framework/input/pending_receiver_retirement.cc" \
  "$root/runtime/framework/input/receiver_retirement_barrier.cc" \
  "$root/runtime/framework/input/receiver_routing_lifecycle.cc" \
  "$root/runtime/framework/input/receiver_endpoint_binding.cc" \
  "$root/tools/tests/channel-endpoint-test.cc" -framework AppKit -o "$stage/test"
"$stage/test" "$@"
