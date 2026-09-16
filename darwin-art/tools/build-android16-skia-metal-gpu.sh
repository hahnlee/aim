#!/bin/bash
set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$script_dir/.." && pwd)"
skia="$root/_aosp/external/skia"
build="$root/_build/skia-metal-gpu"
probe="$build/skia-metal-gpu-smoke"
shadow_skia="$build/source"
coretext_patch="$root/patches/skia/0001-darwin-hwui-disable-coretext-utils.patch"
freetype_archive="$root/_build/graphics-codecs/libft2-darwin.a"
png_archive="$root/_build/graphics-codecs/libpng-darwin.a"
zlib_archive="$root/_build/graphics-codecs/libz-darwin.a"
jpeg_archive="$root/_build/codec-foundation/libjpeg-darwin.a"
libjpeg_root="$root/_aosp/external/libjpeg-turbo"
libwebp_root="$root/_aosp/external/webp"
liblog_archive="$root/_build/graphics-foundations/liblog-darwin.a"
libcutils_archive="$root/_build/graphics-foundations/libcutils-darwin.a"

fail() { echo "skia-metal-gpu: $1" >&2; exit 2; }

[[ "$(uname -s)" == Darwin ]] || fail "requires macOS"
[[ -f "$skia/BUILD.gn" ]] || fail "missing materialized Skia"
[[ "$(tr -d '[:space:]' < "$skia/.source-revision")" == \
   "bcb0f77c44783b1800ba37641ba7ecab04f05e07" ]] || fail "Skia revision drift"
[[ -x "$skia/bin/gn" && -x "$skia/third_party/ninja/ninja" ]] ||
  fail "missing pinned GN/Ninja"
for input in "$coretext_patch" "$freetype_archive" "$png_archive" \
             "$zlib_archive" "$jpeg_archive" "$liblog_archive" \
             "$libcutils_archive"; do
  [[ -f "$input" ]] || fail "missing pinned input $input"
done

# Keep the patched source overlay stable between invocations. Recreating the
# symlink farm invalidates every GN depfile and turns a one-file change into a
# full Skia rebuild.
shadow_identity="$(shasum -a 256 "$skia/BUILD.gn" "$coretext_patch" | shasum -a 256 | awk '{print $1}')"
if [[ ! -f "$shadow_skia/.darwin-art-identity" ]] ||
   [[ "$(cat "$shadow_skia/.darwin-art-identity")" != "$shadow_identity" ]]; then
  rm -rf "$shadow_skia"
  mkdir -p "$shadow_skia"
  while IFS= read -r entry; do
    name="$(basename "$entry")"
    [[ "$name" == BUILD.gn ]] && continue
    if [[ "$name" == third_party ]]; then
      mkdir -p "$shadow_skia/third_party/externals"
      while IFS= read -r third_party_entry; do
        ln -s "$third_party_entry" \
          "$shadow_skia/third_party/$(basename "$third_party_entry")"
      done < <(find "$entry" -mindepth 1 -maxdepth 1 -print | sort)
      ln -s "$libwebp_root" "$shadow_skia/third_party/externals/libwebp"
    else
      ln -s "$entry" "$shadow_skia/$name"
    fi
  done < <(find "$skia" -mindepth 1 -maxdepth 1 -print | sort)
  cp "$skia/BUILD.gn" "$shadow_skia/BUILD.gn"
  patch -d "$shadow_skia" -p1 < "$coretext_patch"
  printf '%s\n' "$shadow_identity" > "$shadow_skia/.darwin-art-identity"
fi

sdk="$(xcrun --sdk macosx --show-sdk-path)"
nativewindow_headers="$root/_aosp/frameworks/native/libs/nativewindow/include"
[[ -d "$nativewindow_headers/android" ]] || fail "missing AOSP nativewindow headers"
mkdir -p "$build"
args="is_official_build=true is_debug=false target_cpu=\"arm64\" \
skia_enable_gpu=true skia_enable_graphite=false skia_enable_pdf=false \
skia_enable_skottie=false skia_enable_svg=false skia_enable_precompile=false \
skia_enable_tools=false skia_enable_android_utils=true \
skia_enable_fontmgr_empty=false skia_enable_fontmgr_custom_empty=true \
skia_enable_fontmgr_custom_embedded=true skia_include_multiframe_procs=true \
skia_use_expat=false skia_use_fonthost_mac=false skia_use_freetype=true \
skia_use_system_freetype2=true \
skia_system_freetype2_include_path=\"$root/_aosp/external/freetype/include\" \
skia_system_freetype2_lib=\"$freetype_archive\" \
skia_use_fontconfig=false skia_use_harfbuzz=false skia_use_icu=false \
skia_use_perfetto=false skia_disable_tracing=false skia_use_gl=true \
skia_gl_standard=\"gles\" skia_use_egl=true skia_use_metal=true \
skia_use_vulkan=false \
skia_use_libjpeg_turbo_decode=true skia_use_libjpeg_turbo_encode=false \
skia_use_no_jpeg_encode=true skia_use_libpng_decode=true \
skia_use_libpng_encode=true skia_use_no_png_encode=false \
skia_use_libwebp_decode=true skia_use_system_libwebp=false \
skia_use_libwebp_encode=false skia_use_no_webp_encode=true \
skia_use_wuffs=false skia_use_piex=false skia_use_xps=false \
skia_use_zlib=true skia_use_system_libpng=true skia_use_system_zlib=true \
skia_use_dng_sdk=false skia_use_libheif=false skia_use_crabbyavif=false \
skia_use_libjxl_decode=false skia_use_libavif=false skia_use_bidi=false \
skia_use_libgrapheme=false skia_build_rust_targets=false \
extra_cflags=[\"-DSK_USER_CONFIG_HEADER=\\\"include/config/SkUserConfigManual.h\\\"\",\"-I$root/_aosp/system/logging/liblog/include\",\"-I$root/_aosp/system/core/libcutils/include\",\"-I$root/_aosp/frameworks/native/opengl/include\",\"-I$nativewindow_headers\",\"-I$libjpeg_root\",\"-I$root/_aosp/external/libpng\",\"-I$root/_aosp/external/zlib\"]"

"$skia/bin/gn" gen "$build" --args="$args" --root="$shadow_skia"
"$skia/third_party/ninja/ninja" -C "$build" skia

archive="$build/libskia.a"
skcms="$build/libskcms.a"
[[ -f "$archive" && -f "$skcms" ]] || fail "GPU Skia archives missing"

# Android's AHardwareBuffer Ganesh extension is selected by is_android in
# upstream GN, while the shared Skia/Metal core must remain a macOS target.
# Compile only that narrow extension with the Android public ABI and append it
# to the canonical archive. The objects are independently cached.
android_gpu_sources=(
  src/gpu/android/AHardwareBufferUtils.cpp
  src/gpu/ganesh/GrAHardwareBufferUtils.cpp
  src/gpu/ganesh/gl/AHardwareBufferGL.cpp
  src/gpu/ganesh/surface/SkSurface_AndroidFactories.cpp
)
android_gpu_dir="$build/android-gpu-objects"
mkdir -p "$android_gpu_dir"
android_gpu_objects=()
for source in "${android_gpu_sources[@]}"; do
  object="$android_gpu_dir/${source//\//_}.o"
  command_file="$object.command"
  source_sha="$(shasum -a 256 "$shadow_skia/$source" | awk '{print $1}')"
  command=(
    clang++ -std=c++17 -O3 -DNDEBUG -arch arm64 -isysroot "$sdk"
    -fPIC -fvisibility=hidden -fvisibility-inlines-hidden -fno-exceptions -fno-rtti
    -DSKIA_IMPLEMENTATION=1 -DSK_GANESH -DSK_GL -DSK_METAL
    -DSK_BUILD_FOR_ANDROID -D__ANDROID_API__=36 '-D__INTRODUCED_IN(x)='
    '-DSK_USER_CONFIG_HEADER="include/config/SkUserConfigManual.h"'
    -I"$shadow_skia" -I"$root/_aosp/system/logging/liblog/include"
    -I"$root/_aosp/system/core/libcutils/include"
    -I"$root/_aosp/frameworks/native/opengl/include"
    -I"$root/_aosp/frameworks/native/libs/arect/include"
    -I"$nativewindow_headers"
    -c "$shadow_skia/$source" -o "$object"
  )
  command_text="$(printf '%q ' "${command[@]}")"
  command_sha="$(printf '%s\n%s\n' "$source_sha" "$command_text" | shasum -a 256 | awk '{print $1}')"
  if [[ ! -f "$object" || ! -f "$command_file" ||
        "$(cat "$command_file" 2>/dev/null || true)" != "$command_sha" ]]; then
    "${command[@]}"
    printf '%s\n' "$command_sha" > "$command_file"
  fi
  android_gpu_objects+=( "$object" )
done
android_gpu_member_names=()
for object in "${android_gpu_objects[@]}"; do
  android_gpu_member_names+=( "$(basename "$object")" )
done
ar -d "$archive" "${android_gpu_member_names[@]}" 2>/dev/null || true
ar -q "$archive" "${android_gpu_objects[@]}"
ranlib "$archive"

clang++ -std=c++20 -O2 -DNDEBUG -arch arm64 -isysroot "$sdk" \
  -fobjc-arc -Wall -Wextra -Werror \
  '-DSK_USER_CONFIG_HEADER="include/config/SkUserConfigManual.h"' \
  -I"$root/_aosp/system/logging/liblog/include" \
  -I"$root/_aosp/system/core/libcutils/include" \
  -I"$shadow_skia" -I"$shadow_skia/include" \
  "$root/probes/skia_metal_gpu_smoke.mm" "$archive" "$skcms" \
  "$freetype_archive" "$png_archive" "$zlib_archive" "$jpeg_archive" \
  "$liblog_archive" "$libcutils_archive" \
  -framework AppKit -framework CoreGraphics -framework CoreFoundation -framework ImageIO \
  -framework Metal -framework QuartzCore -o "$probe"

output="$(DARWIN_ART_SKIA_METAL_HEADLESS=1 "$probe")"
expected_prefix="skia-metal-gpu: frames=8 cpu-readback=0 full-frame-blits=0"
[[ "$output" == "$expected_prefix"* ]] || fail "unexpected smoke output: $output"
echo "$output"
echo "skia-metal-gpu: archive=$archive"
