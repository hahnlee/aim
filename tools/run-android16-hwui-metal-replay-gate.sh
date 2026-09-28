#!/bin/bash
set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$script_dir/.." && pwd)"
build="$root/_build/hwui-metal-replay"
probe="$build/hwui-metal-replay-smoke"
hwui="$root/_build/hwui-static-foundation/libhwui-static-darwin.a"
apex="$root/_build/hwui-static-foundation/libandroid-graphics-apex-common-darwin.a"
skia="$root/_build/skia-metal-gpu/libskia.a"
skcms="$root/_build/skia-metal-gpu/libskcms.a"
skia_utils="$root/_build/skia-hwui-force-load/libskia.a"
log="$root/_build/graphics-foundations/liblog-darwin.a"
cutils="$root/_build/graphics-foundations/libcutils-darwin.a"
utils="$root/_build/graphics-foundations/libutils-darwin.a"
utils_binder="$root/_build/graphics-foundations/libutils-binder-darwin.a"
androidfw="$root/_build/androidfw-foundation/libandroidfw-darwin.a"
minikin="$root/_build/minikin-foundation/libminikin.a"
hostgraphics="$root/_build/hostgraphics/libhostgraphics-darwin.a"
icu_i18n="$root/_build/icu-foundation/libicui18n-darwin.a"
icu_uc="$root/_build/icu-foundation/libicuuc-common-darwin.a"
icu_data="$root/_build/icu-foundation/libicuuc-stubdata-darwin.a"
harfbuzz="$root/_build/harfbuzz-foundation/libharfbuzz_ng-darwin.a"
base="$root/_build/foundation/libandroid-base-darwin.a"
ui_types="$root/_build/ui-types-foundation/libui-types.a"
freetype="$root/_build/graphics-codecs/libft2-darwin.a"
png="$root/_build/graphics-codecs/libpng-darwin.a"
zlib="$root/_build/graphics-codecs/libz-darwin.a"

fail() { echo "hwui-metal-replay: $1" >&2; exit 2; }
[[ "$(uname -s)" == Darwin ]] || fail "requires macOS"
for archive in "$hwui" "$apex" "$skia" "$skcms" "$skia_utils" "$log" "$cutils" "$utils" \
               "$utils_binder" "$androidfw" "$minikin" "$hostgraphics" "$icu_i18n" \
               "$icu_uc" "$icu_data" "$harfbuzz" "$base" "$ui_types" "$freetype" "$png" "$zlib"; do
  [[ -f "$archive" ]] || fail "missing archive $archive"
done

mkdir -p "$build"
sdk="$(xcrun --sdk macosx --show-sdk-path)"
skia_root="$root/_aosp/external/skia"
aosp="$root/_aosp"
deps="$aosp/hwui-static-deps"
clang++ -std=c++20 -O2 -DNDEBUG -arch arm64 -isysroot "$sdk" \
  -fobjc-arc -Wall -Wextra -Wno-unused-parameter \
  -Wno-macro-redefined -Wno-deprecated-literal-operator \
  -Wno-inconsistent-missing-override -Wno-deprecated-declarations \
  '-DSK_USER_CONFIG_HEADER="include/config/SkUserConfigManual.h"' \
  -DHWUI_NULL_GPU=0 -DAIM_HWUI_GPU=1 \
  -I"$skia_root" -I"$skia_root/include" \
  -I"$skia_root/include/core" -I"$skia_root/include/config" \
  -I"$skia_root/include/private" \
  -I"$skia_root/include/utils" \
  -I"$skia_root/include/effects" \
  -I"$skia_root/include/android" \
  -I"$skia_root/include/codec" \
  -I"$aosp/frameworks/base/libs/hwui" \
  -I"$aosp/frameworks/base/libs/hwui/hwui" \
  -I"$aosp/frameworks/base/libs/hwui/pipeline/skia" \
  -I"$aosp/frameworks/base/libs/hwui/jni" \
  -I"$aosp/frameworks/base/libs/hwui/apex/include" \
  -I"$aosp/frameworks/base/libs/androidfw/include" \
  -I"$aosp/frameworks/native/include" -I"$aosp/frameworks/native/include/private" \
  -I"$aosp/frameworks/native/libs/ui/include" \
  -I"$aosp/frameworks/native/libs/ui/include_types" \
  -I"$aosp/frameworks/native/libs/nativebase/include" \
  -I"$aosp/frameworks/native/libs/nativewindow/include" \
  -I"$aosp/frameworks/native/libs/arect/include" \
  -I"$aosp/frameworks/native/libs/math/include" \
  -I"$aosp/frameworks/native/libs/opengl/include" \
  -I"$aosp/frameworks/native/opengl/include" \
  -I"$aosp/frameworks/minikin/include" \
  -I"$deps/frameworks-native/libs/gui/include" \
  -I"$deps/frameworks-native/libs/binder/include" \
  -I"$deps/frameworks-native/libs/nativedisplay/include" \
  -I"$deps/libhardware/include_all" \
  -I"$aosp/system/core/libcutils/include" \
  -I"$aosp/system/libbase/include" -I"$aosp/system/core/libsystem/include" \
  -I"$aosp/system/core/libutils/include" \
  -I"$aosp/system/incremental_delivery/incfs/util/include" \
  -I"$aosp/system/logging/liblog/include" \
  -I"$aosp/external/fmtlib/include" \
  -I"$aosp/external/vulkan-headers/include" \
  -I"$aosp/external/googletest/googletest/include" \
  -I"$aosp/external/harfbuzz_ng/src" \
  -I"$aosp/external/icu-graphics/android_icu4c/include" \
  -I"$aosp/external/icu-graphics/icu4c/source/common" \
  -I"$aosp/libnativehelper-full/include" \
  -I"$aosp/libnativehelper-full/include_platform" \
  -I"$aosp/libnativehelper/header_only_include" \
  -I"$aosp/libnativehelper/include_jni" \
  "$root/probes/hwui_metal_replay_smoke.mm" \
  "$hwui" "$apex" "$skia" "$skcms" "$skia_utils" \
  "$androidfw" "$minikin" "$hostgraphics" "$utils" "$utils_binder" "$cutils" "$log" \
  "$icu_i18n" "$icu_uc" "$icu_data" "$harfbuzz" \
  "$base" "$ui_types" \
  "$freetype" "$png" "$zlib" \
  -framework AppKit -framework CoreGraphics -framework CoreFoundation \
  -framework Metal -framework QuartzCore -framework ImageIO \
  -lc++ -o "$probe"

output="$($probe)"
expected='hwui-metal-replay: frames=8 recording=1 rendernode=1 ripple=1 cpu-readback=0 full-frame-blits=0 drawable-direct=1'
[[ "$output" == "$expected" ]] || fail "unexpected smoke output: $output"
echo "$output"
echo "hwui-metal-replay: archive=$hwui"
freetype="$root/_build/graphics-codecs/libft2-darwin.a"
png="$root/_build/graphics-codecs/libpng-darwin.a"
zlib="$root/_build/graphics-codecs/libz-darwin.a"
