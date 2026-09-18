#!/bin/sh
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
stage=$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-retained-scm-export.XXXXXX")
trap 'rm -rf -- "$stage"' EXIT

xcrun clang++ -std=c++20 -O1 -g -Wall -Wextra -Werror \
  -fsanitize=address,undefined -I"$root/tools/bionic-central-fd-broker/include" \
  -I"$root/tools/bionic-socket-broker-adapter/src" \
  "$root/tools/bionic-central-fd-broker/src/fd_broker.cc" \
  "$root/tools/bionic-socket-broker-adapter/src/retained_scm_export.cc" \
  "$root/tools/tests/retained-scm-export-test.cc" -o "$stage/test"

ASAN_OPTIONS=detect_leaks=0:halt_on_error=1 \
UBSAN_OPTIONS=halt_on_error=1 "$stage/test"
