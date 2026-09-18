#!/bin/sh
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
out=$(mktemp "${TMPDIR:-/tmp}/darwin-art-egl-error-state.XXXXXX")
trap 'rm -f "$out"' EXIT
clang++ -std=c++20 -O1 -g -Wall -Wextra -Werror -pthread \
  -fsanitize=address,undefined -fno-omit-frame-pointer -I"$root" \
  "$root/tools/tests/egl-error-state-test.cc" \
  "$root/compat/graphics/egl_error_state.cc" -o "$out"
ASAN_OPTIONS=detect_leaks=0:halt_on_error=1 \
UBSAN_OPTIONS=halt_on_error=1 "$out"
echo 'EGL error owner: thread-local peek/consume and one-shot PASS'
