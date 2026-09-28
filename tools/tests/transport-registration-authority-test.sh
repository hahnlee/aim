#!/bin/sh
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
stage=$(mktemp -d "${TMPDIR:-/tmp}/transport-registration-authority.XXXXXX")
trap 'rm -rf -- "$stage"' EXIT
clang++ -std=c++20 -O1 -g -Wall -Wextra -Werror -pthread \
  -fsanitize=address,undefined -fno-omit-frame-pointer -I"$root" \
  "$root/runtime/framework/input/transport_registration_authority.cc" \
  "$root/tools/tests/transport-registration-authority-test.cc" -o "$stage/test"
"$stage/test"
echo 'Transport registration authority: exclusivity/successor-intent/quiescence/reentry PASS'
