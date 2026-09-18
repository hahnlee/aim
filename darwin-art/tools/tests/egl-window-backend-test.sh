#!/bin/sh
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
out=$(mktemp "${TMPDIR:-/tmp}/darwin-art-egl-window-backend.XXXXXX")
trap 'rm -f "$out"' EXIT
clang++ -std=c++20 -O1 -g -Wall -Wextra -Werror \
  -fsanitize=address,undefined -fno-omit-frame-pointer -I"$root" \
  "$root/tools/tests/egl-window-backend-test.cc" \
  "$root/compat/graphics/egl_error_state.cc" \
  "$root/compat/graphics/egl_window_backend.cc" \
  "$root/compat/graphics/egl_window_surface_owner.cc" \
  -o "$out"
"$out"
