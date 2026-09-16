#!/bin/bash
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
stage="$(mktemp -d "${TMPDIR:-/tmp}/ndk-image-decoder.XXXXXX")"
trap 'rm -rf -- "$stage"' EXIT
fixture="$root/_aosp/external/skia/resources/images/stoplight.webp"
[[ "$(shasum -a 256 "$fixture" | awk '{print $1}')" == 292753f066add623af1e30bbeeca58f15b3d6d1052e12b13e30f21f8d3c14505 ]]
# Independent reference decoder, coalesced into the NDK's premultiplied RGBA.
magick "$fixture" -coalesce -alpha associate -depth 8 "rgba:$stage/reference.rgba"
xcrun clang++ -std=c++20 -Wall -Wextra -Werror '-D__INTRODUCED_IN(n)=' \
  -I"$root/_aosp/frameworks/native/include" \
  -I"$root/_aosp/frameworks/native/libs/nativewindow/include" \
  -I"$root/_aosp/frameworks/native/libs/arect/include" \
  -I"$root/_aosp/libnativehelper/include_jni" \
  -I"$root/tools/bionic-fs-facade/include" \
  -I"$root/tools/bionic-ioctl-facade/include" \
  "$root/tools/tests/ndk-image-decoder-test.cc" \
  "$root/tools/tests/ndk-image-decoder-fd-test.cc" \
  "$root/tools/tests/ndk-image-decoder-animation-test.cc" -o "$stage/test"
"$stage/test" "$root/_build/runtime-graphics-link-probe/libdarwin_art_runtime_graphics.dylib" \
  "$stage" "$fixture" "$stage/reference.rgba"
