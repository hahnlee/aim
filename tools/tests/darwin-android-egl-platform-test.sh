#!/bin/sh
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
stage=$(mktemp -d "${TMPDIR:-/tmp}/darwin-android-egl-platform.XXXXXX")
trap 'rm -rf -- "$stage"' EXIT

clang++ -std=c++20 -O1 -g -Wall -Wextra -Werror \
  -I"$root" -I"$root/compat" \
  -I"$root/_aosp/libnativehelper/include_jni" \
  "$root/compat/graphics/egl_error_state.cc" \
  "$root/compat/darwin_android_egl_platform.cc" \
  "$root/tools/tests/darwin-android-egl-platform-test.cc" \
  -o "$stage/test"
"$stage/test"
echo 'Darwin EGL platform: initialize/terminate/window shared-owner forwarding PASS'
