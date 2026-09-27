#!/bin/sh
set -eu
repo=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
tmpdir=$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-input-anr.XXXXXX")
trap 'rm -rf "$tmpdir"' EXIT HUP INT TERM
clang++ -std=c++20 -Wall -Wextra -Werror \
  -fsanitize=address,undefined -fno-omit-frame-pointer \
  -I"$repo" -I"$repo/compat" -I"$repo/runtime/framework/input" \
  -I"$repo/tools/bionic-errno-tls/include" \
  "$repo/tools/tests/input-anr-monitor-test.cc" \
  "$repo/runtime/framework/input/finish_ledger.cc" \
  "$repo/runtime/framework/input/receiver_finish_owner.cc" \
  -o "$tmpdir/input-anr-monitor-test"
"$tmpdir/input-anr-monitor-test"
