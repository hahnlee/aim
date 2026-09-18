#!/bin/sh
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
out=$(mktemp "${TMPDIR:-/tmp}/darwin-art-input-domain.XXXXXX")
trap 'rm -f "$out"' EXIT
"${CXX:-clang++}" -std=c++20 -O1 -g -Wall -Wextra -Werror \
  -fsanitize=address,undefined -fno-omit-frame-pointer -I"$root" -I"$root/compat" \
  "$root/tools/tests/input-routing-domain-test.cc" \
  "$root/runtime/framework/input/input_routing_domain.cc" -o "$out"
"$out"
