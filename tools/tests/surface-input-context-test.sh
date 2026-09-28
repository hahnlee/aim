#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
stage="$(mktemp -d "${TMPDIR:-/tmp}/surface-input-context.XXXXXX")"
trap 'rm -rf -- "$stage"' EXIT
cxx="${CXX:-clang++}"
sanitizers="${SANITIZER:-address,undefined}"

"$cxx" -std=c++20 -O1 -g -Wall -Wextra -Werror -pthread \
  "-fsanitize=$sanitizers" -fno-omit-frame-pointer -I"$root" \
  "$root/tools/tests/surface-input-context-test.cc" \
  -o "$stage/surface-input-context-test"

ASAN_OPTIONS=detect_leaks=0:halt_on_error=1 \
UBSAN_OPTIONS=halt_on_error=1 \
  "$stage/surface-input-context-test"

echo 'surface-input-context: PASS (ASan+UBSan, strict C++20, isolated build)'
