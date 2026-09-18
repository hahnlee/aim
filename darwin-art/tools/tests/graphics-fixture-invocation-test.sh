#!/bin/sh
set -eu
repo=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
fixture_tmp=$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-fixture-invocation.XXXXXX")
trap 'rm -rf "$fixture_tmp"' EXIT HUP INT TERM
clang++ -std=c++20 -Wall -Wextra -Werror -fsanitize=address,undefined \
  -fno-omit-frame-pointer -I"$repo" -I"$repo/compat" \
  -I"$repo/_aosp/libnativehelper-full/include_jni" \
  "$repo/tools/tests/graphics-fixture-invocation-test.cc" \
  "$repo/probes/graphics_fixture_state.cc" \
  "$repo/runtime/embedding/graphics_state.cc" -o "$fixture_tmp/test"
"$fixture_tmp/test"
echo 'fixture invocation: nested retirement, deferred JNI cleanup, tombstone reentry PASS'
