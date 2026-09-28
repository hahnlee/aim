#!/bin/bash
set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/.." && pwd)"
skia="$project_root/_aosp/external/skia"
zlib="$project_root/_aosp/external/zlib"
libpng="$project_root/_aosp/external/libpng"
freetype="$project_root/_aosp/external/freetype"
libcutils_include="$project_root/_aosp/system/core/libcutils/include"
liblog_include="$project_root/_aosp/system/logging/liblog/include"
liblog_archive="$project_root/_build/graphics-foundations/liblog-darwin.a"
build_dir="$project_root/_build/skia-text"
codecs_dir="$project_root/_build/graphics-codecs"
freetype_archive="$codecs_dir/libft2-darwin.a"
libpng_archive="$codecs_dir/libpng-darwin.a"
zlib_archive="$codecs_dir/libz-darwin.a"
roboto="$skia/resources/fonts/Roboto-Regular.ttf"
codecs_lock="$project_root/upstream/android16-graphics-codecs.lock"
sources_lock="$project_root/sources.lock"

expected_skia_revision=bcb0f77c44783b1800ba37641ba7ecab04f05e07
expected_skia_build_sha=4485d6db39c678711d097a8adcc7eb9a88c9f91e8c0ed02fae83a392584a1a4a
expected_freetype_gn_sha=cc044527b27b91cef3b7da863ee461d9473cb7f1f3e315de13c2d324cb809ce3
expected_output='Skia Android text raster: Click glyphs=5 ink=1097 rowBytes=768 hash=1f94df6816828ca2'

fail_input() {
  echo "skia-text-raster: $1" >&2
  exit 2
}

sha256() {
  shasum -a 256 "$1" | awk '{print $1}'
}

lock_value() {
  local lock="$1"
  local key="$2"
  sed -n "s/^${key}=//p" "$lock" | tail -1
}

verify_hash() {
  local path="$1"
  local expected="$2"
  [[ -f "$path" ]] || fail_input "missing pinned input: $path"
  local actual
  actual="$(sha256 "$path")"
  if [[ "$actual" != "$expected" ]]; then
    echo "skia-text-raster: pinned input mismatch: $path" >&2
    echo "expected=$expected actual=$actual" >&2
    exit 3
  fi
}

[[ -f "$sources_lock" ]] || fail_input "missing $sources_lock"
[[ -f "$codecs_lock" ]] || fail_input "missing $codecs_lock"
[[ -f "$skia/.source-revision" ]] || fail_input "missing Skia revision marker"
if [[ "$(tr -d '[:space:]' < "$skia/.source-revision")" != "$expected_skia_revision" ]]; then
  fail_input "Skia must be platform/external/skia@$expected_skia_revision"
fi
if [[ "$(lock_value "$sources_lock" SKIA_REVISION)" != "$expected_skia_revision" ]]; then
  fail_input "sources.lock SKIA_REVISION does not select $expected_skia_revision"
fi
verify_hash "$skia/BUILD.gn" "$expected_skia_build_sha"
verify_hash "$skia/third_party/freetype2/BUILD.gn" "$expected_freetype_gn_sha"

for key in ZLIB_REVISION ZLIB_ANDROID_BP_SHA256 ZLIB_PUBLIC_HEADER_SHA256 \
           LIBPNG_REVISION LIBPNG_ANDROID_BP_SHA256 LIBPNG_PUBLIC_HEADER_SHA256 \
           FREETYPE_REVISION FREETYPE_ANDROID_BP_SHA256 \
           FREETYPE_PUBLIC_HEADER_SHA256 ROBOTO_REGULAR_SHA256; do
  [[ -n "$(lock_value "$codecs_lock" "$key")" ]] || fail_input "missing $key in $codecs_lock"
done
verify_hash "$zlib/Android.bp" "$(lock_value "$codecs_lock" ZLIB_ANDROID_BP_SHA256)"
verify_hash "$zlib/zlib.h" "$(lock_value "$codecs_lock" ZLIB_PUBLIC_HEADER_SHA256)"
verify_hash "$libpng/Android.bp" "$(lock_value "$codecs_lock" LIBPNG_ANDROID_BP_SHA256)"
verify_hash "$libpng/png.h" "$(lock_value "$codecs_lock" LIBPNG_PUBLIC_HEADER_SHA256)"
verify_hash "$freetype/Android.bp" "$(lock_value "$codecs_lock" FREETYPE_ANDROID_BP_SHA256)"
verify_hash "$freetype/include/freetype/freetype.h" \
  "$(lock_value "$codecs_lock" FREETYPE_PUBLIC_HEADER_SHA256)"
verify_hash "$roboto" "$(lock_value "$codecs_lock" ROBOTO_REGULAR_SHA256)"
for project_revision in "$zlib:ZLIB_REVISION" "$libpng:LIBPNG_REVISION" \
                        "$freetype:FREETYPE_REVISION"; do
  source_dir="${project_revision%%:*}"
  revision_key="${project_revision#*:}"
  if [[ -f "$source_dir/.source-revision" ]] &&
     [[ "$(tr -d '[:space:]' < "$source_dir/.source-revision")" != \
        "$(lock_value "$codecs_lock" "$revision_key")" ]]; then
    fail_input "$source_dir revision marker does not match $codecs_lock"
  fi
done

for archive in "$freetype_archive" "$libpng_archive" "$zlib_archive"; do
  [[ -f "$archive" ]] || {
    echo "skia-text-raster: missing pinned Android graphics archive: $archive" >&2
    echo "run tools/build-android16-graphics-codecs.sh first" >&2
    exit 2
  }
  [[ "$(lipo -archs "$archive")" == arm64 ]] || fail_input "archive is not Darwin arm64: $archive"
done
ar="$(xcrun --find ar)"
for archive_members in "$freetype_archive:26" "$libpng_archive:18" "$zlib_archive:19"; do
  archive="${archive_members%%:*}"
  expected_members="${archive_members#*:}"
  actual_members="$({ "$ar" -t "$archive" || true; } | grep -v '^__\.SYMDEF' | wc -l | tr -d ' ')"
  [[ "$actual_members" == "$expected_members" ]] || fail_input "unexpected module member count: $archive expected=$expected_members actual=$actual_members"
done
[[ -d "$liblog_include" ]] || fail_input "missing pinned AOSP liblog headers: $liblog_include"
[[ -f "$liblog_archive" ]] || fail_input "missing pinned AOSP liblog archive: $liblog_archive"
[[ "$(lipo -archs "$liblog_archive")" == arm64 ]] || fail_input "archive is not Darwin arm64: $liblog_archive"
for pair in "$freetype_archive:_FT_Load_Glyph" \
            "$libpng_archive:_png_read_info" "$zlib_archive:_inflate"; do
  archive="${pair%%:*}"
  symbol="${pair#*:}"
  if ! nm -gU "$archive" | grep -F " T $symbol" >/dev/null; then
    fail_input "archive lacks representative definition $symbol: $archive"
  fi
done

gn="$skia/bin/gn"
ninja="$skia/third_party/ninja/ninja"
[[ -x "$gn" ]] || fail_input "missing pinned Skia GN binary: $gn"
[[ -x "$ninja" ]] || fail_input "missing pinned Skia Ninja binary: $ninja"
verify_hash "$gn" "$(lock_value "$sources_lock" SKIA_GN_DARWIN_ARM64_SHA256)"
verify_hash "$ninja" "$(lock_value "$sources_lock" SKIA_NINJA_DARWIN_ARM64_SHA256)"

mkdir -p "$build_dir"
gn_args="is_official_build=true is_debug=false target_cpu=\"arm64\" \
skia_enable_gpu=false skia_enable_graphite=false skia_enable_pdf=false \
skia_enable_skottie=false skia_enable_svg=false skia_enable_precompile=false \
skia_enable_tools=false skia_enable_fontmgr_empty=false \
skia_enable_fontmgr_custom_empty=false skia_enable_fontmgr_custom_directory=false \
skia_enable_fontmgr_custom_embedded=true skia_enable_fontmgr_android=false \
skia_enable_fontmgr_android_ndk=false skia_use_expat=false \
skia_use_fonthost_mac=false skia_use_freetype=true skia_use_system_freetype2=true \
skia_system_freetype2_include_path=\"$freetype/include\" \
skia_system_freetype2_lib=\"$freetype_archive\" \
skia_use_fontconfig=false skia_use_harfbuzz=false skia_use_icu=false \
skia_use_perfetto=false skia_disable_tracing=false skia_use_gl=false \
skia_use_metal=false skia_use_vulkan=false skia_use_libjpeg_turbo_decode=false \
skia_use_libjpeg_turbo_encode=false skia_use_no_jpeg_encode=true \
skia_use_libpng_decode=false skia_use_libpng_encode=false \
skia_use_no_png_encode=true skia_use_libwebp_decode=false \
skia_use_libwebp_encode=false skia_use_no_webp_encode=true skia_use_wuffs=false \
skia_use_piex=false skia_use_xps=false skia_use_zlib=false \
skia_use_dng_sdk=false skia_use_libheif=false skia_use_crabbyavif=false \
skia_use_libjxl_decode=false skia_use_libavif=false skia_use_bidi=false \
skia_use_libgrapheme=false skia_build_rust_targets=false \
extra_cflags=[\"-I$libcutils_include\",\"-I$liblog_include\",\"-DSK_USER_CONFIG_HEADER=\\\"include/config/SkUserConfigManual.h\\\"\"]"

(
  cd "$skia"
  "$gn" gen "$build_dir" --args="$gn_args"
)
"$ninja" -C "$build_dir" skia

args_file="$build_dir/args.gn"
required_args=(
  'skia_enable_fontmgr_empty = false'
  'skia_enable_fontmgr_custom_empty = false'
  'skia_enable_fontmgr_custom_embedded = true'
  'skia_use_fonthost_mac = false'
  'skia_use_freetype = true'
  'skia_use_system_freetype2 = true'
  "skia_system_freetype2_include_path = \"$freetype/include\""
  "skia_system_freetype2_lib = \"$freetype_archive\""
)
for required_arg in "${required_args[@]}"; do
  grep -Fx "$required_arg" "$args_file" >/dev/null || fail_input "generated args.gn lacks: $required_arg"
done
grep -F 'SK_USER_CONFIG_HEADER=' "$args_file" >/dev/null || fail_input "generated args.gn lacks upstream Android SkUserConfig selection"
if grep -F "$project_root/compat" "$args_file" >/dev/null; then
  fail_input "compat headers are forbidden in the text-raster gate"
fi
freetype_desc="$(cd "$skia" && "$gn" desc "$build_dir" //third_party/freetype2:freetype2 libs)"
if ! grep -F "$freetype_archive" <<<"$freetype_desc" >/dev/null; then
  echo "skia-text-raster: GN did not preserve the exact AOSP FreeType archive in //third_party/freetype2:freetype2.libs" >&2
  echo "GN output: $freetype_desc" >&2
  echo "required narrow upstream patch: add a path-valued skia_system_freetype2_archive arg and append it to system(\"freetype2\").libs" >&2
  exit 2
fi

skia_archive="$build_dir/libskia.a"
skcms_archive="$build_dir/libskcms.a"
[[ -f "$skia_archive" && -f "$skcms_archive" ]] || fail_input "Skia text archives were not generated"
skia_symbols="$(nm -gU "$skia_archive" | c++filt)"
for symbol in 'SkFontMgr_New_Custom_Data' 'SkScalerContext_FreeType::generateImage'; do
  grep -F "$symbol" <<<"$skia_symbols" >/dev/null || fail_input "libskia.a lacks FreeType text symbol: $symbol"
done

cxx="$(command -v clang++)"
probe="$build_dir/skia-text-raster-smoke"
link_map="$build_dir/skia-text-raster-smoke.map"
"$cxx" -std=c++20 -arch arm64 -O2 -DNDEBUG -Wall -Wextra -Werror \
  '-DSK_USER_CONFIG_HEADER="include/config/SkUserConfigManual.h"' \
  -I"$liblog_include" -I"$skia" \
  "$project_root/probes/skia_text_raster_smoke.cc" \
  "$skia_archive" "$skcms_archive" \
  "$freetype_archive" "$libpng_archive" "$zlib_archive" "$liblog_archive" \
  -framework CoreGraphics -framework CoreFoundation \
  -Wl,-map,"$link_map" -o "$probe"

for archive in "$freetype_archive" "$libpng_archive" "$zlib_archive" "$liblog_archive"; do
  grep -F "$archive(" "$link_map" >/dev/null || fail_input "link map did not consume pinned archive: $archive"
done
dependencies="$(otool -L "$probe")"
if grep -E '/opt/homebrew|/usr/local|CoreText|libfreetype|libpng|libz\.[0-9]*\.dylib' \
     <<<"$dependencies" >/dev/null; then
  echo "skia-text-raster: forbidden host text/codec dependency" >&2
  echo "$dependencies" >&2
  exit 4
fi

output="$("$probe" "$roboto")"
if [[ "$output" != "$expected_output" ]]; then
  echo "skia-text-raster: deterministic Roboto pixel output changed" >&2
  echo "expected: $expected_output" >&2
  echo "actual:   $output" >&2
  exit 4
fi

echo "skia-text-raster: $output"
echo "skia-text-raster: archive=$skia_archive FreeType=$freetype_archive"
