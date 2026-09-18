#!/bin/sh
set -eu
repo=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
fixture_tmp=$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-fixture-exchange.XXXXXX")
trap 'rm -rf "$fixture_tmp"' EXIT HUP INT TERM
clang++ -std=c++20 -Wall -Wextra -Werror -fsanitize=address,undefined \
  -fno-omit-frame-pointer -I"$repo" -I"$repo/compat" \
  "$repo/tools/tests/fixture-input-exchange-test.cc" -o "$fixture_tmp/test"
"$fixture_tmp/test"
echo 'fixture input exchange: partial writes/ACKs, retained deadline, exact uint64 sequence, malformed/EOF PASS'
