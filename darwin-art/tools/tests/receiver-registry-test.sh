#!/bin/sh
set -eu

repo_dir=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
build_dir=$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-receiver-registry.XXXXXX")
trap 'rm -rf "$build_dir"' EXIT HUP INT TERM

cxx=${CXX:-c++}
"$cxx" -std=c++20 -Wall -Wextra -Werror -pthread \
  -DDARWIN_ART_RECEIVER_REGISTRY_TESTING \
  -I"$repo_dir" \
  "$repo_dir/runtime/framework/input/receiver_registry.cc" \
  "$repo_dir/tools/tests/receiver-registry-test.cc" \
  -o "$build_dir/receiver-registry-test"
"$build_dir/receiver-registry-test"
echo "receiver-registry-test: PASS"
