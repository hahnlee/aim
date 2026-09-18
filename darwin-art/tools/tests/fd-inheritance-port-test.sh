#!/bin/sh
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
stage=$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-fd-inheritance.XXXXXX")
trap 'rm -rf -- "$stage"' EXIT

clang++ -std=c++20 -Wall -Wextra -Werror -O1 -g \
  -fsanitize=address,undefined -fno-omit-frame-pointer -pthread \
  -I"$root" \
  "$root/tools/bionic-socket-broker-adapter/src/fd_inheritance.cc" \
  "$root/tools/tests/fd-inheritance-port-test.cc" \
  -o "$stage/test"

ASAN_OPTIONS=detect_leaks=0:halt_on_error=1 \
UBSAN_OPTIONS=halt_on_error=1 "$stage/test"
