#!/bin/sh
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo_root=$(CDPATH= cd -- "$script_dir/../.." && pwd)
tmp_root=$(mktemp -d "${TMPDIR:-/tmp}/wire-channel-lifetime.XXXXXX")
trap 'rm -rf "$tmp_root"' EXIT HUP INT TERM

clang++ -std=c++20 -Wall -Wextra -Werror -pedantic -pthread \
  -I"$repo_root" \
  "$repo_root/compat/binder/wire_channel_lifetime.cc" \
  "$script_dir/wire-channel-lifetime-test.cc" \
  -o "$tmp_root/wire-channel-lifetime-test"

"$tmp_root/wire-channel-lifetime-test"
echo 'wire-channel-lifetime: PASS (strict C++20, pthread, isolated build)'
