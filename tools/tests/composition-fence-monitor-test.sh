#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
stage="$(mktemp -d "${TMPDIR:-/tmp}/composition-fence-monitor.XXXXXX")"
trap 'rm -rf -- "$stage"' EXIT

sdk="$(xcrun --sdk macosx --show-sdk-path)"
cxx="$(xcrun --find clang++)"
flags=(
  -arch arm64
  -isysroot "$sdk"
  -std=c++20
  -O1
  -g
  -Wall
  -Wextra
  -Werror
  -fsanitize=address,undefined
  -fno-omit-frame-pointer
  -I"$root"
  -I"$root/tools/bionic-socket-broker-adapter/include"
  -pthread
)

"$cxx" "${flags[@]}" \
  "$root/compat/window/composition_fence_monitor.cc" \
  "$root/tools/tests/composition-fence-monitor-test.cc" \
  -o "$stage/composition-fence-monitor-test"

ASAN_OPTIONS=detect_leaks=0:halt_on_error=1 \
UBSAN_OPTIONS=halt_on_error=1 \
  "$stage/composition-fence-monitor-test"
