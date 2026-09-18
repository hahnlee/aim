#!/bin/sh
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
stage=$(mktemp -d /tmp/darwin-art-scm-exports.XXXXXX)
trap 'rm -rf -- "$stage"' EXIT
xcrun clang++ -std=c++20 -O1 -g -Wall -Wextra -Werror \
  -fsanitize=address,undefined -I"$root" \
  "$root/tools/bionic-socket-broker-adapter/src/android_scm_exports.cc" \
  "$root/tools/tests/android-scm-exports-test.cc" -o "$stage/test"
ASAN_OPTIONS=detect_leaks=0:halt_on_error=1 \
UBSAN_OPTIONS=halt_on_error=1 "$stage/test"
