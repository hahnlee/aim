#!/bin/sh
set -eu

repo_dir=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
build_dir=$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-wire-connection-registry.XXXXXX")
trap 'rm -rf "$build_dir"' EXIT HUP INT TERM

cxx=${CXX:-c++}
"$cxx" -std=c++20 -Wall -Wextra -Werror -pthread \
  -I"$repo_dir" \
  "$repo_dir/compat/binder/wire_channel_lifetime.cc" \
  "$repo_dir/tools/tests/wire-connection-registry-test.cc" \
  -o "$build_dir/wire-connection-registry-test"
"$build_dir/wire-connection-registry-test"
echo "wire-connection-registry-test: PASS"
