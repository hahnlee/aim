#!/bin/bash
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
stage="$(mktemp -d "${TMPDIR:-/tmp}/ndk-bitmap-test.XXXXXX")"
trap 'rm -rf -- "$stage"' EXIT
xcrun clang++ -std=c++20 -Wall -Wextra -Werror '-D__INTRODUCED_IN(n)=' \
  -I"$root/_aosp/frameworks/native/include" \
  -I"$root/_aosp/frameworks/native/libs/nativewindow/include" \
  -I"$root/_aosp/libnativehelper/include_jni" \
  "$root/tools/tests/ndk-bitmap-test.cc" \
  -framework CoreGraphics -framework ImageIO -framework CoreFoundation \
  -o "$stage/test"
"$stage/test" "$root/_build/runtime-graphics-link-probe/libdarwin_art_runtime_graphics.dylib"
