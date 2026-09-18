#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
stage="$(mktemp -d "${TMPDIR:-/tmp}/blast-callback-gate.XXXXXX")"
trap 'rm -rf -- "$stage"' EXIT

sdk="$(xcrun --sdk macosx --show-sdk-path)"
cxx="$(xcrun --find clang++)"
sanitizers="${SANITIZER:-address,undefined}"
flags=(
  -arch arm64
  -isysroot "$sdk"
  -std=c++20
  -O1
  -g
  -Wall
  -Wextra
  -Werror
  -pthread
  "-fsanitize=$sanitizers"
  -fno-omit-frame-pointer
  -I"$root"
)

# This intentionally links only the header-only gate test; no JNI, BLAST,
# runtime, or shared product build is part of this ownership check.
"$cxx" "${flags[@]}" "$root/tools/tests/blast-callback-gate-test.cc" \
  -o "$stage/blast-callback-gate-test"
ASAN_OPTIONS=detect_leaks=0:halt_on_error=1 \
UBSAN_OPTIONS=halt_on_error=1 \
  "$stage/blast-callback-gate-test"
