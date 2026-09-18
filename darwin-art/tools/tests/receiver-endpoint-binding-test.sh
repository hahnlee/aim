#!/bin/sh
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
out=$(mktemp "${TMPDIR:-/tmp}/darwin-art-receiver-endpoint-binding.XXXXXX")
trap 'rm -f "$out"' EXIT
clang++ -std=c++20 -O1 -g -Wall -Wextra -Werror \
  -fsanitize=address,undefined -fno-omit-frame-pointer \
  -I"$root" -I"$root/compat" \
  -I"$root/_aosp/frameworks/native/include" \
  "$root/tools/tests/receiver-endpoint-binding-test.cc" \
  "$root/runtime/framework/input/transport_registration_authority.cc" \
  "$root/compat/looper/reusable_task.cc" \
  "$root/runtime/framework/input/claimed_input_transport_pump.cc" \
  "$root/runtime/framework/input/receiver_endpoint_binding.cc" \
  "$root/runtime/framework/input/input_transport_pump.cc" \
  "$root/runtime/framework/input/input_transport_readiness.cc" \
  "$root/runtime/framework/input/input_transport.cc" \
  "$root/runtime/framework/input/input_framed_reader.cc" \
  "$root/runtime/framework/input/input_resource_progress.cc" \
  -o "$out"
"$out"
