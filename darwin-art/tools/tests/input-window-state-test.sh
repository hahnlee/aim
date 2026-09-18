#!/bin/sh
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
stage=$(mktemp -d "${TMPDIR:-/tmp}/input-window-state.XXXXXX")
trap 'rm -rf -- "$stage"' EXIT
clang++ -std=c++20 -O1 -g -fsanitize=address,undefined \
  -Wall -Wextra -Werror -I"$root" \
  "$root/tools/tests/input-window-state-test.cc" -o "$stage/test"
"$stage/test"
echo 'InputWindowState provenance/eligibility/invalid-frame revocation: PASS'
