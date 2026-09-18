#!/bin/sh
set -eu

repo_dir=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
build_dir=$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-main-task-drain.XXXXXX")
trap 'rm -rf "$build_dir"' EXIT HUP INT TERM

if [ "$(uname -s)" != Darwin ]; then
  echo "main-task-drain: SKIP (macOS-only CoreFoundation test)"
  exit 0
fi

rustc --edition=2024 -C debuginfo=1 \
  "$repo_dir/tools/tests/main-task-drain-test.rs" \
  -o "$build_dir/main-task-drain-test" \
  -C link-arg=-framework -C link-arg=CoreFoundation \
  -C link-arg=-framework -C link-arg=Foundation \
  -C link-arg=-lobjc
"$build_dir/main-task-drain-test"
