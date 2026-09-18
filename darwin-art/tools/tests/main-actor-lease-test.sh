#!/bin/sh
set -eu
repo_dir=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
build_dir=$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-main-actor-lease.XXXXXX")
trap 'rm -rf "$build_dir"' EXIT HUP INT TERM
if [ "$(uname -s)" != Darwin ]; then
  echo 'main actor lease: SKIP (macOS only)'
  exit 0
fi
rustc --edition=2024 "$repo_dir/tools/tests/main-actor-lease-test.rs" \
  -o "$build_dir/main-actor-lease-test"
"$build_dir/main-actor-lease-test"
