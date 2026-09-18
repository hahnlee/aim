#!/bin/sh
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
cxx=${CXX:-clang++}
out=$(mktemp "${TMPDIR:-/tmp}/darwin-art-finish-ledger.XXXXXX")
trap 'rm -f "$out"' EXIT

"$cxx" -std=c++20 -O1 -g -Wall -Wextra -Werror \
  -fsanitize=address,undefined -fno-omit-frame-pointer \
  -I"$root" -I"$root/compat" \
  "$root/tools/tests/finish-ledger-test.cc" \
  "$root/runtime/framework/input/finish_ledger.cc" -o "$out"
"$out"
