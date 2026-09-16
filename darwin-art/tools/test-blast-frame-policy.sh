#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
output="$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-blast-frame.XXXXXX")"
trap 'rm -rf "$output"' EXIT

clang++ -std=c++20 -Wall -Wextra -Werror \
  "$root/tools/tests/blast-frame-policy-test.cc" \
  -o "$output/test"
"$output/test"
