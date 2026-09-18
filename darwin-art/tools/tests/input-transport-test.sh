#!/bin/sh
set -eu
repo=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
tmpdir=$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-input-transport.XXXXXX")
trap 'rm -rf "$tmpdir"' EXIT HUP INT TERM
clang++ -std=c++20 -Wall -Wextra -Werror \
  -fsanitize=address,undefined -fno-omit-frame-pointer \
  -I"$repo" -I"$repo/compat" -I"$repo/runtime/framework/input" \
  -I"$repo/tools/bionic-errno-tls/include" \
  "$repo/tools/tests/input-transport-test.cc" \
  "$repo/runtime/framework/input/transport_registration_authority.cc" \
  "$repo/runtime/framework/input/input_transport.cc" \
  "$repo/runtime/framework/input/input_framed_reader.cc" \
  "$repo/runtime/framework/input/input_resource_progress.cc" \
  "$repo/runtime/framework/input/input_transport_pump.cc" \
  "$repo/runtime/framework/input/input_transport_readiness.cc" \
  "$repo/runtime/framework/input/finish_ledger.cc" \
  "$repo/runtime/framework/input/receiver_finish_owner.cc" \
  -o "$tmpdir/input-transport-test"
"$tmpdir/input-transport-test"
