#!/bin/bash
set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/.." && pwd)"
skia="$project_root/_aosp/external/skia"
lock_file="$project_root/upstream/android16-skia-hwui-force-load.lock"
sources_lock="$project_root/sources.lock"
build_dir="$project_root/_build/skia-hwui-force-load"
shadow_skia="$build_dir/source"
coretext_patch="$project_root/patches/skia/0001-darwin-hwui-disable-coretext-utils.patch"
cross_tu_abi_patch="$project_root/patches/skia/0002-darwin-hwui-export-cross-tu-abi.patch"
codecs_dir="$project_root/_build/graphics-codecs"
freetype="$project_root/_aosp/external/freetype"
freetype_archive="$codecs_dir/libft2-darwin.a"
libpng_archive="$codecs_dir/libpng-darwin.a"
zlib_archive="$codecs_dir/libz-darwin.a"
libjpeg_root="$project_root/_aosp/external/libjpeg-turbo"
libwebp_root="$project_root/_aosp/external/webp"
libjpeg_archive="$project_root/_build/codec-foundation/libjpeg-darwin.a"
liblog_archive="$project_root/_build/graphics-foundations/liblog-darwin.a"
libcutils_archive="$project_root/_build/graphics-foundations/libcutils-darwin.a"
libcutils_include="$project_root/_aosp/system/core/libcutils/include"
liblog_include="$project_root/_aosp/system/logging/liblog/include"

# shellcheck disable=SC1090
source "$lock_file"

fail_input() {
  echo "skia-hwui-force-load: $1" >&2
  exit 2
}

verify_sha() {
  local file="$1" expected="$2" actual
  [[ -f "$file" ]] || fail_input "missing pinned input $file"
  actual="$(shasum -a 256 "$file" | awk '{print $1}')"
  [[ "$actual" == "$expected" ]] ||
    fail_input "checksum mismatch file=$file expected=$expected actual=$actual"
}

[[ -f "$skia/.source-revision" ]] || fail_input "missing Skia revision marker"
[[ "$(tr -d '[:space:]' < "$skia/.source-revision")" == "$SKIA_REVISION" ]] ||
  fail_input "Skia revision mismatch expected=$SKIA_REVISION"
grep -Fx "SKIA_REVISION=$SKIA_REVISION" "$sources_lock" >/dev/null ||
  fail_input "sources.lock does not select the pinned Skia revision"
verify_sha "$skia/BUILD.gn" "$SKIA_BUILD_GN_SHA256"
verify_sha "$skia/gn/skia.gni" "$SKIA_GNI_SHA256"
verify_sha "$skia/gn/utils.gni" "$SKIA_UTILS_GNI_SHA256"
verify_sha "$skia/include/private/base/SkFeatures.h" "$SKIA_FEATURES_SHA256"
verify_sha "$skia/src/ports/SkFontMgr_custom_empty.cpp" "$SKIA_CUSTOM_EMPTY_SHA256"
verify_sha "$skia/tools/SkSharingProc.cpp" "$SKIA_SHARING_PROC_SHA256"
verify_sha "$skia/client_utils/android/BitmapRegionDecoder.cpp" \
  "$SKIA_BITMAP_REGION_DECODER_SHA256"
verify_sha "$skia/client_utils/android/FrontBufferedStream.cpp" \
  "$SKIA_FRONT_BUFFERED_STREAM_SHA256"
[[ -f "$libwebp_root/.source-revision" ]] || fail_input "missing WebP revision marker"
[[ "$(tr -d '[:space:]' < "$libwebp_root/.source-revision")" == "$WEBP_REVISION" ]] ||
  fail_input "WebP revision mismatch expected=$WEBP_REVISION"
verify_sha "$libwebp_root/Android.bp" "$WEBP_ANDROID_BP_SHA256"
verify_sha "$libwebp_root/src/webp/decode.h" "$WEBP_DECODE_HEADER_SHA256"
verify_sha "$coretext_patch" "$DARWIN_CORETEXT_PATCH_SHA256"
verify_sha "$cross_tu_abi_patch" "$SKIA_CROSS_TU_ABI_PATCH_SHA256"

required_archives=("$freetype_archive" "$libpng_archive" "$zlib_archive" \
  "$libjpeg_archive" \
  "$liblog_archive" "$libcutils_archive")
for archive in "${required_archives[@]}"; do
  [[ -f "$archive" ]] || fail_input "missing module archive $archive"
  [[ "$(lipo -archs "$archive")" == arm64 ]] || fail_input "not arm64: $archive"
done
[[ -d "$freetype/include" && -d "$libcutils_include" && -d "$liblog_include" ]] ||
  fail_input "missing pinned AOSP exported headers"

gn="$skia/bin/gn"
ninja="$skia/third_party/ninja/ninja"
[[ -x "$gn" && -x "$ninja" ]] || fail_input "missing pinned GN/Ninja"

mkdir -p "$build_dir"
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
  elif [[ "$name" == include ]]; then
    # Headers are a patch boundary. Keep a real copy so provider-only
    # visibility annotations cannot mutate or follow the pristine checkout.
    cp -R "$entry" "$shadow_skia/$name"
  else
    ln -s "$entry" "$shadow_skia/$name"
  fi
done < <(find "$skia" -mindepth 1 -maxdepth 1 -print | sort)
cp "$skia/BUILD.gn" "$shadow_skia/BUILD.gn"
patch -d "$shadow_skia" -p1 < "$coretext_patch"
patch -d "$shadow_skia" -p1 < "$cross_tu_abi_patch"

gn_args="is_official_build=true is_debug=false target_cpu=\"arm64\" \
skia_enable_gpu=false skia_enable_graphite=false skia_enable_pdf=false \
skia_enable_skottie=false skia_enable_svg=false skia_enable_precompile=false \
skia_enable_tools=false skia_enable_android_utils=true skia_enable_fontmgr_empty=false \
skia_enable_fontmgr_custom_empty=true skia_enable_fontmgr_custom_directory=false \
skia_enable_fontmgr_custom_embedded=true skia_enable_fontmgr_android=false \
skia_enable_fontmgr_android_ndk=false skia_include_multiframe_procs=true \
skia_use_expat=false skia_use_fonthost_mac=false skia_use_freetype=true \
skia_use_system_freetype2=true \
skia_system_freetype2_include_path=\"$freetype/include\" \
skia_system_freetype2_lib=\"$freetype_archive\" \
skia_use_fontconfig=false skia_use_harfbuzz=false skia_use_icu=false \
skia_use_perfetto=false skia_disable_tracing=false skia_use_gl=false \
skia_use_metal=false skia_use_vulkan=false skia_use_libjpeg_turbo_decode=true \
skia_use_libjpeg_turbo_encode=false skia_use_no_jpeg_encode=true \
skia_use_libpng_decode=true skia_use_libpng_encode=false \
skia_use_no_png_encode=true skia_use_libwebp_decode=true \
skia_use_libwebp_encode=false skia_use_no_webp_encode=true skia_use_wuffs=false \
skia_use_system_libwebp=false \
skia_use_piex=false skia_use_xps=false skia_use_zlib=true \
skia_use_system_libpng=true skia_use_system_zlib=true \
skia_use_dng_sdk=false skia_use_libheif=false skia_use_crabbyavif=false \
skia_use_libjxl_decode=false skia_use_libavif=false skia_use_bidi=false \
skia_use_libgrapheme=false skia_build_rust_targets=false \
extra_cflags=[\"-I$libcutils_include\",\"-I$liblog_include\",\"-I$libjpeg_root\",\"-I$project_root/_aosp/external/libpng\",\"-I$project_root/_aosp/external/zlib\",\"-DSK_USER_CONFIG_HEADER=\\\"include/config/SkUserConfigManual.h\\\"\"]"

(cd "$shadow_skia" && "$gn" gen "$build_dir" --args="$gn_args")
"$ninja" -C "$build_dir" skia

args_file="$build_dir/args.gn"
for required in \
  'skia_enable_android_utils = true' \
  'skia_enable_fontmgr_custom_empty = true' \
  'skia_include_multiframe_procs = true' \
  'skia_use_fonthost_mac = false' \
  'skia_use_freetype = true' \
  'skia_use_libjpeg_turbo_decode = true' \
  'skia_use_libpng_decode = true' \
  'skia_use_libwebp_decode = true' \
  'skia_use_system_libwebp = false' \
  'skia_use_system_libpng = true' \
  'skia_use_system_zlib = true'; do
  grep -Fx "$required" "$args_file" >/dev/null || fail_input "missing GN selection: $required"
done
resolved_sources="$(cd "$shadow_skia" && "$gn" desc "$build_dir" //:skia sources)"
resolved_android_sources="$(cd "$shadow_skia" && "$gn" desc "$build_dir" //:android_utils sources)"
for android_source in \
  '//client_utils/android/BitmapRegionDecoder.cpp' \
  '//client_utils/android/FrontBufferedStream.cpp'; do
  [[ "$(grep -Fc "$android_source" <<<"$resolved_android_sources")" == 1 ]] ||
    fail_input "Android utils source missing or duplicated: $android_source"
done
if grep -E 'SkCTFont(\.cpp|CreateExactCopy\.cpp)' <<<"$resolved_sources" >/dev/null; then
  fail_input "patched GN source closure still contains CoreText utilities"
fi

archive="$build_dir/libskia.a"
skcms="$build_dir/libskcms.a"
[[ -f "$archive" && -f "$skcms" ]] || fail_input "Skia archives not generated"
[[ "$(lipo -archs "$archive")" == arm64 ]] || fail_input "Skia archive is not arm64"
archive_members="$(ar -t "$archive" | grep -v '^__\.SYMDEF' | wc -l | tr -d ' ')"
[[ "$archive_members" == "$EXPECTED_SKIA_ARCHIVE_MEMBER_COUNT" ]] ||
  fail_input "Skia archive members=$archive_members expected=$EXPECTED_SKIA_ARCHIVE_MEMBER_COUNT"

definitions="$(nm -gUC "$archive")"
for symbol in \
  'SkFontMgr_New_Custom_Empty()' \
  'SkSharingSerialContext::serializeImage(' \
  'SkSharingSerialContext::setDirectContext('; do
  grep -F " T $symbol" <<<"$definitions" >/dev/null ||
    fail_input "missing upstream definition $symbol"
done
android_utils_symbols="$build_dir/android-utils-defined-symbols.txt"
raw_definitions="$(nm -gU "$archive")"
for symbol in \
  '__ZN7android4skia19BitmapRegionDecoder12decodeRegionEP8SkBitmapPNS0_12BRDAllocatorERK7SkIRecti11SkColorTypeb5sk_spI12SkColorSpaceE' \
  '__ZN7android4skia19BitmapRegionDecoder4MakeE5sk_spI6SkDataE' \
  '__ZN7android4skia19BitmapRegionDecoderC1ENSt3__110unique_ptrI14SkAndroidCodecNS2_14default_deleteIS4_EEEE' \
  '__ZN7android4skia19FrontBufferedStream4MakeENSt3__110unique_ptrI8SkStreamNS2_14default_deleteIS4_EEEEm' \
  '__ZNK7android4skia19BitmapRegionDecoder5widthEv' \
  '__ZNK7android4skia19BitmapRegionDecoder6heightEv'; do
  grep -F " T $symbol" <<<"$raw_definitions" >/dev/null ||
    fail_input "missing Android utils definition $symbol"
  printf '%s\n' "$symbol"
done > "$android_utils_symbols"
android_utils_symbols_sha="$(shasum -a 256 "$android_utils_symbols" | awk '{print $1}')"
[[ "$android_utils_symbols_sha" == "$ANDROID_UTILS_SYMBOL_MANIFEST_SHA256" ]] ||
  fail_input "Android utils symbol manifest changed: $android_utils_symbols_sha"
if nm -u "$archive" | grep -E '_(CTFont|kCTFont)' >/dev/null; then
  fail_input "archive still imports CoreText"
fi

cxx="$(command -v clang++)"
probe="$build_dir/skia-hwui-force-load-smoke"
link_map="$build_dir/skia-hwui-force-load-smoke.map"
"$cxx" -std=c++20 -arch arm64 -O2 -DNDEBUG -Wall -Wextra -Werror \
  '-DSK_USER_CONFIG_HEADER="include/config/SkUserConfigManual.h"' \
  -I"$liblog_include" -I"$skia" \
  "$project_root/probes/skia_hwui_force_load_smoke.cc" \
  -Wl,-force_load,"$archive" "$skcms" "$freetype_archive" \
  "$libpng_archive" "$zlib_archive" "$libjpeg_archive" \
  "$liblog_archive" "$libcutils_archive" \
  -framework CoreGraphics -framework CoreFoundation -framework ImageIO \
  -Wl,-map,"$link_map" -o "$probe"

for consumed_archive in "$freetype_archive" "$libpng_archive" "$zlib_archive" \
                        "$libjpeg_archive" \
                        "$liblog_archive" "$libcutils_archive"; do
  grep -F "$consumed_archive(" "$link_map" >/dev/null ||
    fail_input "force-load executable did not consume $consumed_archive"
done
if grep -E 'CoreText|/opt/homebrew|/usr/local' "$link_map" >/dev/null; then
  fail_input "forbidden host provider in force-load link map"
fi
output="$("$probe")"
[[ "$output" == "$EXPECTED_SMOKE" ]] ||
  fail_input "unexpected smoke output: $output"

dependencies="$(otool -L "$probe")"
if grep -E 'CoreText|/opt/homebrew|/usr/local|libfreetype|libpng|libz\.[0-9]*\.dylib' \
     <<<"$dependencies" >/dev/null; then
  echo "$dependencies" >&2
  fail_input "forbidden host dependency in force-load executable"
fi
if nm -u "$probe" | grep -E '_(CTFont|kCTFont)' >/dev/null; then
  fail_input "force-load executable still imports CoreText symbols"
fi
provider_audit="$build_dir/forbidden-host-provider-audit.txt"
printf 'CoreText=0\nHomebrew=0\n' > "$provider_audit"
provider_audit_sha="$(shasum -a 256 "$provider_audit" | awk '{print $1}')"
[[ "$provider_audit_sha" == "$FORBIDDEN_HOST_PROVIDER_AUDIT_SHA256" ]] ||
  fail_input "forbidden-provider audit manifest changed: $provider_audit_sha"

echo "skia-hwui-force-load: $output"
echo "skia-hwui-force-load: archive=$archive members=$archive_members android-utils=6"
echo "skia-hwui-force-load: CoreText=0 Homebrew=0"
