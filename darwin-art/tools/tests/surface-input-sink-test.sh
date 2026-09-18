#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
stage="$(mktemp -d "${TMPDIR:-/tmp}/surface-input-sink.XXXXXX")"
trap 'rm -rf -- "$stage"' EXIT
cxx="${CXX:-clang++}"
"$cxx" -std=c++20 -O1 -g -Wall -Wextra -Werror -pthread \
  -I"$root/compat" -I"$root/probes" \
  "$root/tools/tests/surface-input-sink-test.cc" \
  -o "$stage/surface-input-sink-test"
"$stage/surface-input-sink-test"
