#!/bin/sh
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
stage=$(mktemp -d "${TMPDIR:-/tmp}/input-resource-progress.XXXXXX")
trap 'rm -rf -- "$stage"' EXIT

clang++ -std=c++20 -O1 -g -Wall -Wextra -Werror -Wpedantic -pthread \
  -fsanitize=address,undefined -fno-omit-frame-pointer -I"$root" \
  "$root/runtime/framework/input/input_resource_progress.cc" \
  "$root/tools/tests/input-resource-progress-test.cc" -o "$stage/test"
ASAN_OPTIONS=detect_leaks=0:halt_on_error=1 \
  UBSAN_OPTIONS=halt_on_error=1 "$stage/test"
echo 'Input resource progress source: weak subscription/reentry/overlap/OOM/zero-allocation Notify PASS'
