#!/bin/bash
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
stage="$(mktemp -d "${TMPDIR:-/tmp}/native-window-observer.XXXXXX")"
trap 'rm -rf -- "$stage"' EXIT
xcrun clang++ -std=c++20 -Wall -Wextra -Werror -fsanitize=address,undefined \
  -I"$root/_build/nativehelper-foundation/source/libnativehelper/include_jni" \
  -I"$root/_aosp/android16-surfaceflinger-core/buffer-native/libs/nativewindow/include" \
  -I"$root/_aosp/android16-surfaceflinger-core/buffer-native/libs/nativebase/include" \
  -I"$root/_aosp/frameworks/native/libs/nativebase/include" \
  -I"$root/_aosp/system/core/libcutils/include" \
  -I"$root/_aosp/system/core/libsystem/include" \
  -I"$root/_aosp/frameworks/native/libs/arect/include" \
  "$root/tools/tests/native-window-observer-test.cc" \
  "$root/_build/runtime-graphics-link-probe/libdarwin_art_runtime_graphics.dylib" \
  -Wl,-rpath,"$root/_build/runtime-graphics-link-probe" -o "$stage/test"
"$stage/test"
