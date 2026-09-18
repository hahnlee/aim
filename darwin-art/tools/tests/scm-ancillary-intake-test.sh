#!/bin/sh
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
stage=$(mktemp -d /tmp/darwin-art-scm-ancillary-intake.XXXXXX)
trap 'rm -rf -- "$stage"' EXIT
sdk=$(xcrun --sdk macosx --show-sdk-path)
xcrun clang++ -arch arm64 -isysroot "$sdk" -std=c++20 -O1 -g \
  -Wall -Wextra -Werror -fsanitize=address,undefined -I"$root" \
  "$root/tools/bionic-socket-broker-adapter/src/ancillary_intake.cc" \
  "$root/tools/bionic-socket-broker-adapter/src/fd_inheritance.cc" \
  "$root/tools/tests/scm-ancillary-intake-test.cc" -o "$stage/test"
ASAN_OPTIONS=detect_leaks=0:halt_on_error=1 \
UBSAN_OPTIONS=halt_on_error=1 "$stage/test"
