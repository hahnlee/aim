#!/bin/bash
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
stage="$(mktemp -d "${TMPDIR:-/tmp}/darwin-process-config.XXXXXX")"
trap 'rm -rf -- "$stage"' EXIT
xcrun clang++ -std=c++20 -Wall -Wextra -Werror -fsanitize=address,undefined \
  -I"$root" -I"$root/include" \
  "$root/tools/tests/process-config-test.cc" \
  "$root/runtime/embedding/process_config.cc" \
  -o "$stage/test"
"$stage/test"
