#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
work_dir="$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-binder-wire-policy.XXXXXX")"
trap 'rm -rf "$work_dir"' EXIT

"${CXX:-clang++}" -std=c++20 -Wall -Wextra -Werror \
  -I"$repo_root" \
  "$repo_root/tools/tests/binder-wire-dispatch-policy-test.cc" \
  -o "$work_dir/binder-wire-dispatch-policy-test"
"$work_dir/binder-wire-dispatch-policy-test"

echo "binder-wire-dispatch-policy: PASS"
