#!/bin/sh
set -eu

repo_dir=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
build_dir=$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-receiver-admission.XXXXXX")
trap 'rm -rf "$build_dir"' EXIT HUP INT TERM

cxx=${CXX:-c++}
"$cxx" -std=c++20 -Wall -Wextra -Werror -pthread \
  -I"$repo_dir" \
  "$repo_dir/runtime/framework/input/receiver_admission.cc" \
  "$repo_dir/runtime/framework/input/receiver_registry.cc" \
  "$repo_dir/tools/tests/receiver-admission-test.cc" \
  -o "$build_dir/receiver-admission-test"
"$build_dir/receiver-admission-test"
echo "receiver-admission-test: PASS"
