#!/bin/sh
set -eu
repo_dir=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
build_dir=$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-appkit-actor.XXXXXX")
trap 'rm -rf "$build_dir"' EXIT HUP INT TERM
if [ "$(uname -s)" != Darwin ]; then
  echo 'AppKit actor owner: SKIP (macOS only)'
  exit 0
fi
rustc --edition=2024 "$repo_dir/tools/tests/appkit-actor-owner-test.rs" \
  -o "$build_dir/appkit-actor-owner-test"
"$build_dir/appkit-actor-owner-test"
