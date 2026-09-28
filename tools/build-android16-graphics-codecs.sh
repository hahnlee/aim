#!/bin/bash
set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/.." && pwd)"
lock_file="$project_root/upstream/android16-graphics-codecs.lock"
external_root="${AIM_ANDROID16_EXTERNAL_ROOT:-$project_root/_aosp/external}"
build_dir="$project_root/_build/graphics-codecs"
object_dir="$build_dir/objects"

# The lock is a trusted, assignment-only repository file.
# shellcheck disable=SC1090
source "$lock_file"

zlib="$external_root/zlib"
libpng="$external_root/libpng"
freetype="$external_root/freetype"
roboto="$project_root/_aosp/external/skia/resources/fonts/Roboto-Regular.ttf"

report_missing() {
  local destination="$1"
  local project="$2"
  local revision="$3"
  local manifest_sha="$4"
  echo "graphics-codecs: missing source destination=$destination" >&2
  echo "  project=$project subtree=. revision=$revision Android.bp.sha256=$manifest_sha" >&2
}

missing=0
if [[ ! -d "$zlib" ]]; then
  report_missing "$zlib" "$ZLIB_PROJECT" "$ZLIB_REVISION" "$ZLIB_ANDROID_BP_SHA256"
  missing=1
fi
if [[ ! -d "$libpng" ]]; then
  report_missing "$libpng" "$LIBPNG_PROJECT" "$LIBPNG_REVISION" "$LIBPNG_ANDROID_BP_SHA256"
  missing=1
fi
if [[ ! -d "$freetype" ]]; then
  report_missing "$freetype" "$FREETYPE_PROJECT" "$FREETYPE_REVISION" "$FREETYPE_ANDROID_BP_SHA256"
  missing=1
fi
if (( missing != 0 )); then
  echo "graphics-codecs: materialize the pinned Gitiles archives without .git metadata; see upstream/android16-graphics-codecs.md" >&2
  exit 2
fi

verify_sha256() {
  local path="$1"
  local expected="$2"
  local actual
  if [[ ! -f "$path" ]]; then
    echo "graphics-codecs: missing pinned file $path" >&2
    exit 2
  fi
  actual="$(shasum -a 256 "$path" | awk '{print $1}')"
  if [[ "$actual" != "$expected" ]]; then
    echo "graphics-codecs: checksum mismatch $path" >&2
    echo "expected=$expected actual=$actual" >&2
    exit 2
  fi
}

verify_source() {
  local directory="$1"
  local revision="$2"
  local manifest_sha="$3"
  local public_header="$4"
  local public_header_sha="$5"
  if [[ -e "$directory/.git" ]]; then
    echo "graphics-codecs: Git metadata is forbidden in $directory" >&2
    exit 2
  fi
  if [[ -f "$directory/.source-revision" ]] &&
      [[ "$(tr -d '[:space:]' < "$directory/.source-revision")" != "$revision" ]]; then
    echo "graphics-codecs: revision marker mismatch $directory" >&2
    exit 2
  fi
  verify_sha256 "$directory/Android.bp" "$manifest_sha"
  verify_sha256 "$directory/$public_header" "$public_header_sha"
}

verify_source "$zlib" "$ZLIB_REVISION" "$ZLIB_ANDROID_BP_SHA256" \
  zlib.h "$ZLIB_PUBLIC_HEADER_SHA256"
verify_source "$libpng" "$LIBPNG_REVISION" "$LIBPNG_ANDROID_BP_SHA256" \
  png.h "$LIBPNG_PUBLIC_HEADER_SHA256"
verify_source "$freetype" "$FREETYPE_REVISION" "$FREETYPE_ANDROID_BP_SHA256" \
  include/freetype/freetype.h "$FREETYPE_PUBLIC_HEADER_SHA256"
verify_sha256 "$roboto" "$ROBOTO_REGULAR_SHA256"

# Android.bp `libz_srcs`, plus arm64 and target.darwin_arm64 cflags.
zlib_sources=(
  adler32.c adler32_simd.c compress.c cpu_features.c crc32.c
  crc32_simd.c crc_folding.c deflate.c gzclose.c gzlib.c gzread.c
  gzwrite.c infback.c inffast.c inflate.c inftrees.c trees.c uncompr.c
  zutil.c
)

# Android.bp `srcs: ["*.c"]`, excluding example.c/pngtest.c, plus arm64 C
# sources. `arm/filter_neon.S` is explicitly excluded for arm64.
libpng_sources=(
  png.c pngerror.c pngget.c pngmem.c pngpread.c pngread.c pngrio.c
  pngrtran.c pngrutil.c pngset.c pngtrans.c pngwio.c pngwrite.c pngwtran.c
  pngwutil.c
  arm/arm_init.c arm/filter_neon_intrinsics.c arm/palette_neon_intrinsics.c
)

# Android.bp `libft2_defaults.srcs`; libft2 adds PNG/Zlib feature definitions
# and target.not_windows PIC flags.
freetype_sources=(
  src/autofit/autofit.c
  src/base/ftbase.c src/base/ftbbox.c src/base/ftbitmap.c src/base/ftdebug.c
  src/base/ftfstype.c src/base/ftgasp.c src/base/ftglyph.c src/base/ftinit.c
  src/base/ftmm.c src/base/ftstroke.c src/base/fttype1.c src/base/ftsystem.c
  src/cid/type1cid.c src/cff/cff.c src/gzip/ftgzip.c src/psaux/psaux.c
  src/pshinter/pshinter.c src/psnames/psnames.c src/raster/raster.c
  src/sdf/sdf.c src/sfnt/sfnt.c src/smooth/smooth.c src/svg/svg.c
  src/truetype/truetype.c src/type1/type1.c
)

verify_sources() {
  local directory="$1"
  shift
  local source
  for source in "$@"; do
    if [[ ! -f "$directory/$source" ]]; then
      echo "graphics-codecs: Android.bp-selected source missing: $directory/$source" >&2
      exit 2
    fi
  done
}
verify_sources "$zlib" "${zlib_sources[@]}"
verify_sources "$libpng" "${libpng_sources[@]}"
verify_sources "$freetype" "${freetype_sources[@]}"

cc="$(command -v clang)"
if command -v llvm-ar >/dev/null 2>&1; then
  ar="$(command -v llvm-ar)"
else
  ar="$(xcrun --find ar)"
fi

common_flags=(-arch arm64 -fPIC -fvisibility=hidden)
mkdir -p "$object_dir/zlib" "$object_dir/libpng" "$object_dir/freetype"

object_path() {
  local module="$1"
  local source="$2"
  local name="${source//\//_}"
  echo "$object_dir/$module/${name%.*}.o"
}

zlib_objects=()
for source in "${zlib_sources[@]}"; do
  object="$(object_path zlib "$source")"
  echo "graphics-codecs: compile zlib/$source"
  "$cc" -std=gnu17 "${common_flags[@]}" -I"$zlib" \
    -DHAVE_HIDDEN -DZLIB_CONST -DCHROMIUM_ZLIB_NO_CASTAGNOLI \
    -DADLER32_SIMD_NEON -DCRC32_ARMV8_CRC32 -DINFLATE_CHUNK_READ_64LE \
    -DARMV8_OS_MACOS -O3 -Wall -Werror \
    -Wno-deprecated-non-prototype -Wno-unused -Wno-unused-parameter \
    -c "$zlib/$source" -o "$object"
  zlib_objects+=("$object")
done

libpng_objects=()
for source in "${libpng_sources[@]}"; do
  object="$(object_path libpng "$source")"
  echo "graphics-codecs: compile libpng/$source"
  "$cc" -std=gnu89 "${common_flags[@]}" -I"$libpng" -I"$zlib" \
    -O3 -Wall -Werror -Wno-unused-parameter \
    -c "$libpng/$source" -o "$object"
  libpng_objects+=("$object")
done

freetype_objects=()
for source in "${freetype_sources[@]}"; do
  object="$(object_path freetype "$source")"
  echo "graphics-codecs: compile freetype/$source"
  "$cc" -std=gnu17 "${common_flags[@]}" \
    -I"$freetype/include" -I"$freetype" -I"$libpng" -I"$zlib" \
    -DDARWIN_NO_CARBON -DFT2_BUILD_LIBRARY \
    -DFT_CONFIG_OPTION_USE_PNG -DFT_CONFIG_OPTION_USE_ZLIB \
    -DFT_CONFIG_OPTION_SYSTEM_ZLIB -DPIC -O2 -W -Wall -Werror \
    -Wno-unused-parameter -Wno-unused-variable -Wno-unused-function \
    -c "$freetype/$source" -o "$object"
  freetype_objects+=("$object")
done

archive_dir="$(mktemp -d "$build_dir/archive.XXXXXX")"
zlib_archive_staged="$archive_dir/libz-darwin.a"
libpng_archive_staged="$archive_dir/libpng-darwin.a"
freetype_archive_staged="$archive_dir/libft2-darwin.a"
"$ar" rcs "$zlib_archive_staged" "${zlib_objects[@]}"
"$ar" rcs "$libpng_archive_staged" "${libpng_objects[@]}"
"$ar" rcs "$freetype_archive_staged" "${freetype_objects[@]}"

zlib_archive="$build_dir/libz-darwin.a"
libpng_archive="$build_dir/libpng-darwin.a"
freetype_archive="$build_dir/libft2-darwin.a"
mv "$zlib_archive_staged" "$zlib_archive"
mv "$libpng_archive_staged" "$libpng_archive"
mv "$freetype_archive_staged" "$freetype_archive"
rmdir "$archive_dir"

for object in "${zlib_objects[@]}" "${libpng_objects[@]}" "${freetype_objects[@]}"; do
  kind="$(file "$object")"
  if [[ "$kind" != *"Mach-O 64-bit object arm64"* ]]; then
    echo "graphics-codecs: unexpected object format: $kind" >&2
    exit 3
  fi
done
for archive in "$zlib_archive" "$libpng_archive" "$freetype_archive"; do
  if [[ "$(lipo -archs "$archive")" != arm64 ]]; then
    echo "graphics-codecs: archive is not arm64: $archive" >&2
    exit 3
  fi
done

archive_members() {
  "$ar" -t "$1" | grep -v '^__\.SYMDEF'
}
verify_members() {
  local archive="$1"
  shift
  local expected actual
  expected="$(printf '%s\n' "$@" | sort)"
  actual="$(archive_members "$archive" | sort)"
  if [[ "$actual" != "$expected" ]]; then
    echo "graphics-codecs: archive member manifest mismatch: $archive" >&2
    exit 3
  fi
}
zlib_members=()
for source in "${zlib_sources[@]}"; do
  object="$(object_path zlib "$source")"
  zlib_members+=("${object##*/}")
done
libpng_members=()
for source in "${libpng_sources[@]}"; do
  object="$(object_path libpng "$source")"
  libpng_members+=("${object##*/}")
done
freetype_members=()
for source in "${freetype_sources[@]}"; do
  object="$(object_path freetype "$source")"
  freetype_members+=("${object##*/}")
done
verify_members "$zlib_archive" "${zlib_members[@]}"
verify_members "$libpng_archive" "${libpng_members[@]}"
verify_members "$freetype_archive" "${freetype_members[@]}"

verify_definitions() {
  local archive="$1"
  shift
  local definitions symbol
  definitions="$(nm -gU "$archive")"
  for symbol in "$@"; do
    if ! grep -F " T $symbol" <<<"$definitions" >/dev/null; then
      echo "graphics-codecs: missing definition $symbol in $archive" >&2
      exit 4
    fi
  done
}
verify_definitions "$zlib_archive" _inflate _deflate _crc32
verify_definitions "$libpng_archive" _png_create_read_struct _png_read_info _png_write_info
verify_definitions "$freetype_archive" _FT_Init_FreeType _FT_New_Face _FT_Load_Glyph

glyph_smoke="$build_dir/freetype-glyph-smoke"
"$cc" -std=c17 -arch arm64 -Wall -Wextra -Werror \
  -I"$freetype/include" "$project_root/probes/freetype_glyph_smoke.c" \
  "$freetype_archive" "$libpng_archive" "$zlib_archive" -o "$glyph_smoke"
glyph_output="$("$glyph_smoke" "$roboto")"
expected_glyph_output='FreeType Android glyph: C 19x23 advance=1344 hash=0b4b6c1a74bd39d0'
if [[ "$glyph_output" != "$expected_glyph_output" ]]; then
  echo "graphics-codecs: unexpected glyph raster output" >&2
  echo "  expected: $expected_glyph_output" >&2
  echo "  actual:   $glyph_output" >&2
  exit 4
fi

echo "graphics-codecs: zlib=${#zlib_objects[@]} libpng=${#libpng_objects[@]} freetype=${#freetype_objects[@]}"
echo "graphics-codecs: arm64 archives=$zlib_archive,$libpng_archive,$freetype_archive"
echo "graphics-codecs: $glyph_output"
