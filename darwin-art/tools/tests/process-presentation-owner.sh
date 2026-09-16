#!/bin/bash
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
stage="$(mktemp -d "${TMPDIR:-/tmp}/process-presentation-owner.XXXXXX")"
trap 'rm -rf -- "$stage"' EXIT
xcrun clang++ -std=c++20 -Wall -Wextra -Werror -I"$root" \
  "$root/tools/tests/process-presentation-owner-test.cc" -o "$stage/test"
"$stage/test"
