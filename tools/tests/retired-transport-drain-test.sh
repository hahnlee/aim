#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
stage="$(mktemp -d "${TMPDIR:-/tmp}/retired-transport-drain.XXXXXX")"
trap 'rm -rf -- "$stage"' EXIT
clang++ -std=c++20 -O1 -g -Wall -Wextra -Werror \
  -fsanitize=address,undefined -fno-omit-frame-pointer \
  -I"$root" -I"$root/compat" \
  "$root/tools/tests/retired-transport-drain-test.cc" \
  "$root/runtime/framework/input/transport_registration_authority.cc" \
  "$root/runtime/framework/input/retired_transport_drain.cc" \
  "$root/runtime/framework/input/input_transport.cc" \
  "$root/runtime/framework/input/input_framed_reader.cc" \
  "$root/runtime/framework/input/input_resource_progress.cc" \
  -o "$stage/test"
"$stage/test"
