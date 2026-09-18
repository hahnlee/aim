#!/bin/sh
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
stage=$(mktemp -d "${TMPDIR:-/tmp}/claimed-pump-retry.XXXXXX")
trap 'find "$stage" -depth -delete' EXIT
clang++ -std=c++20 -O1 -g -Wall -Wextra -Werror -pthread \
  -fsanitize=address,undefined -fno-omit-frame-pointer \
  -I"$root" -I"$root/compat" -I"$root/_aosp/frameworks/native/include" \
  "$root/compat/looper/reusable_task.cc" \
  "$root/runtime/framework/input/transport_registration_authority.cc" \
  "$root/runtime/framework/input/input_transport.cc" \
  "$root/runtime/framework/input/input_framed_reader.cc" \
  "$root/runtime/framework/input/input_resource_progress.cc" \
  "$root/runtime/framework/input/input_transport_pump.cc" \
  "$root/runtime/framework/input/input_transport_readiness.cc" \
  "$root/runtime/framework/input/claimed_input_transport_pump.cc" \
  "$root/tools/tests/claimed-pump-retire-retry-test.cc" -o "$stage/test"
"$stage/test"
