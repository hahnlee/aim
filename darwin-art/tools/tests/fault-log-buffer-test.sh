#!/bin/bash
set -euo pipefail
test_root="$(cd "$(dirname "$0")/../.." && pwd)"
cxx_path="$(xcrun --find clang++)"
sdk_path="$(xcrun --sdk macosx --show-sdk-path)"
stage="$(mktemp -d "${TMPDIR:-/tmp}/fault-log-buffer.XXXXXX")"
trap 'rm -rf -- "$stage"' EXIT
"$cxx_path" -isysroot "$sdk_path" -std=c++17 -O1 -g -Wall -Wextra -Werror -Wpedantic \
  -I"$test_root" \
  "$test_root/tools/tests/fault-log-buffer-test.cc" \
  -o "$stage/fault-log-buffer-test"
"$stage/fault-log-buffer-test"
