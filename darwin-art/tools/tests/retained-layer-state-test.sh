#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
stage="$(mktemp -d "${TMPDIR:-/tmp}/retained-layer-state.XXXXXX")"
trap 'rm -rf -- "$stage"' EXIT

xcrun clang++ -std=c++20 -O1 -g -Wall -Wextra -Werror \
  -I"$root" -I"$root/compat" \
  "$root/compat/surfaceflinger/retained_layer_state.cc" \
  "$root/tools/tests/retained-layer-state-test.cc" \
  -o "$stage/test"

"$stage/test"
