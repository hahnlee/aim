#!/bin/bash
set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/.." && pwd)"
build_dir="$project_root/_build/prefix-resolver"
rust_archive="$project_root/target/debug/libaim_prefix.a"

cd "$project_root"
cargo build -p aim-prefix
mkdir -p "$build_dir"

xcrun clang -std=c17 -arch arm64 -Wall -Wextra -Werror \
  -I"$project_root/include" \
  "$project_root/probes/prefix_resolver_smoke.c" \
  "$rust_archive" -framework Security -framework CoreFoundation \
  -o "$build_dir/prefix-resolver-smoke"

file "$build_dir/prefix-resolver-smoke" | grep -F 'Mach-O 64-bit executable arm64' >/dev/null
"$build_dir/prefix-resolver-smoke"
