#!/bin/bash
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
ndk="${ANDROID_NDK_HOME:-$HOME/Library/Android/sdk/ndk/28.2.13676358}"
export DARWIN_ART_ANGLE_DIRECTORY="${DARWIN_ART_ANGLE_DIRECTORY:-$root/_build/angle-source/out/DarwinArtRelease}"
test -f "$DARWIN_ART_ANGLE_DIRECTORY/libEGL.dylib"
test -f "$DARWIN_ART_ANGLE_DIRECTORY/libGLESv2.dylib"
stage="$(mktemp -d "${TMPDIR:-/tmp}/image-reader-ndk-runtime.XXXXXX")"
trap 'rm -rf -- "$stage"' EXIT
xcrun clang++ -std=c++20 -Wall -Wextra -Werror -Wno-nullability-completeness \
  -fsanitize=address,undefined -framework Foundation \
  -I"$root/_aosp/frameworks/native/libs/nativewindow/include" \
  -I"$root/_aosp/frameworks/native/include" \
  -I"$root/_aosp/frameworks/native/libs/arect/include" \
  -I"$root/_aosp/system/core/libsystem/include" \
  -idirafter "$ndk/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include" \
  "$root/tools/tests/image-reader-ndk-runtime-test.mm" \
  "$root/_build/runtime-graphics-link-probe/libdarwin_art_runtime_graphics.dylib" \
  -Wl,-rpath,"$root/_build/runtime-graphics-link-probe" -o "$stage/test"
"$stage/test"
