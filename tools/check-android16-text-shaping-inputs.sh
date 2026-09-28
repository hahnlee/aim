#!/bin/bash
set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/.." && pwd)"
aosp_root="$project_root/_aosp"
lock_file="$project_root/upstream/android16-hwui.lock"

lock_value() {
  local key="$1"
  awk -F= -v key="$key" \
    '$1 == key { print substr($0, index($0, "=") + 1); exit }' "$lock_file"
}

failures=()

record_failure() {
  failures+=("$1")
}

check_lock() {
  local project="$1"
  local key="$2"
  local expected="$3"
  local actual
  actual="$(lock_value "$key")"
  if [[ "$actual" != "$expected" ]]; then
    record_failure "$project: $key expected=$expected actual=${actual:-<missing>}"
  fi
}

check_sha256() {
  local project="$1"
  local revision="$2"
  local file="$3"
  local expected="$4"
  if [[ ! -f "$file" ]]; then
    record_failure "$project@$revision: missing $file (sha256=$expected)"
    return
  fi
  local actual
  actual="$(shasum -a 256 "$file" | awk '{print $1}')"
  if [[ "$actual" != "$expected" ]]; then
    record_failure "$project@$revision: checksum $file expected=$expected actual=$actual"
  fi
}

check_source_list() {
  local project="$1"
  local revision="$2"
  local source_root="$3"
  shift 3
  if [[ ! -d "$source_root" ]]; then
    record_failure "$project@$revision: missing source directory $source_root"
    return
  fi
  local source
  for source in "$@"; do
    if [[ ! -f "$source_root/$source" ]]; then
      record_failure "$project@$revision: missing source $source_root/$source"
    fi
  done
}

check_glob_count() {
  local project="$1"
  local revision="$2"
  local source_dir="$3"
  local suffix="$4"
  local expected="$5"
  if [[ ! -d "$source_dir" ]]; then
    record_failure "$project@$revision: missing source directory $source_dir"
    return
  fi
  local actual
  actual="$(find "$source_dir" -maxdepth 1 -type f -name "*$suffix" | wc -l | tr -d ' ')"
  if [[ "$actual" != "$expected" ]]; then
    record_failure "$project@$revision: $source_dir/*$suffix expected=$expected actual=$actual"
  fi
}

minikin_revision="1e1d5d137d487df875d7db69b5ff24e7d0291612"
harfbuzz_revision="e489c416b6f8d2a9a2e0e85b781d1e4a0c431401"
icu_revision="f17caeafcf20bd38074a9963c31df3629b70b5f5"
freetype_revision="d968d2541f7158e18ab22680bfa08a538019bf6a"
skia_revision="bcb0f77c44783b1800ba37641ba7ecab04f05e07"

check_lock platform/frameworks/minikin MINIKIN_REVISION "$minikin_revision"
check_lock platform/external/harfbuzz_ng HARFBUZZ_NG_REVISION "$harfbuzz_revision"
check_lock platform/external/icu ICU_REVISION "$icu_revision"
check_lock platform/external/freetype FREETYPE_REVISION "$freetype_revision"
check_lock platform/external/skia SKIA_REVISION "$skia_revision"

minikin_root="$aosp_root/frameworks/minikin"
minikin_module="$minikin_root/libs/minikin"
harfbuzz_root="$aosp_root/external/harfbuzz_ng"
icu_root="$aosp_root/external/icu-graphics"
freetype_root="$aosp_root/external/freetype"

check_sha256 platform/frameworks/minikin "$minikin_revision" \
  "$minikin_root/Android.bp" \
  93fc02b83bdf1ab57335d88d4be8d30937a422784e16691b2c812917a5982cb2
check_sha256 platform/frameworks/minikin "$minikin_revision" \
  "$minikin_module/Android.bp" \
  8b90ecd388ff80df2dc9cb5bc62421d1e8da68191fb2e4316f1a9ed96f340a01
check_sha256 platform/external/harfbuzz_ng "$harfbuzz_revision" \
  "$harfbuzz_root/Android.bp" \
  02393c97deee24420fee50100c9beccd5a998bade71b260d170fcdac9ae831f2

minikin_sources=(
  BidiUtils.cpp
  CmapCoverage.cpp
  Emoji.cpp
  Font.cpp
  FontCollection.cpp
  FontFakery.cpp
  FontFamily.cpp
  FontFeatureUtils.cpp
  FontFileParser.cpp
  FontUtils.cpp
  GraphemeBreak.cpp
  GreedyLineBreaker.cpp
  Hyphenator.cpp
  HyphenatorMap.cpp
  Layout.cpp
  LayoutCore.cpp
  LayoutUtils.cpp
  LineBreaker.cpp
  LineBreakerUtil.cpp
  Locale.cpp
  LocaleListCache.cpp
  MeasuredText.cpp
  Measurement.cpp
  MinikinFontFactory.cpp
  MinikinInternal.cpp
  OptimalLineBreaker.cpp
  ScriptUtils.cpp
  SparseBitSet.cpp
  SystemFonts.cpp
  U16StringPiece.cpp
  WordBreaker.cpp
)
check_source_list platform/frameworks/minikin "$minikin_revision" \
  "$minikin_module" "${minikin_sources[@]}"

harfbuzz_sources=(
  hb-aat-layout.cc hb-aat-map.cc hb-blob.cc hb-buffer-serialize.cc
  hb-buffer-verify.cc hb-buffer.cc hb-common.cc hb-draw.cc
  hb-face-builder.cc hb-face.cc hb-fallback-shape.cc hb-font.cc hb-map.cc
  hb-number.cc hb-ot-cff1-table.cc hb-ot-cff2-table.cc hb-ot-color.cc
  hb-ot-face.cc hb-ot-font.cc hb-ot-layout.cc hb-ot-map.cc hb-ot-math.cc
  hb-ot-meta.cc hb-ot-metrics.cc hb-ot-name.cc hb-ot-shape-fallback.cc
  hb-ot-shape-normalize.cc hb-ot-shape.cc hb-ot-shaper-arabic.cc
  hb-ot-shaper-default.cc hb-ot-shaper-hangul.cc hb-ot-shaper-hebrew.cc
  hb-ot-shaper-indic-table.cc hb-ot-shaper-indic.cc hb-ot-shaper-khmer.cc
  hb-ot-shaper-myanmar.cc hb-ot-shaper-syllabic.cc hb-ot-shaper-thai.cc
  hb-ot-shaper-use.cc hb-ot-shaper-vowel-constraints.cc hb-ot-tag.cc
  hb-ot-var.cc hb-outline.cc hb-paint-extents.cc hb-paint.cc hb-set.cc
  hb-shape-plan.cc hb-shape.cc hb-shaper.cc hb-static.cc hb-style.cc
  hb-ucd.cc hb-unicode.cc
)
check_source_list platform/external/harfbuzz_ng "$harfbuzz_revision" \
  "$harfbuzz_root/src" "${harfbuzz_sources[@]}"

# Host ICU is not just its public headers. These three manifests define the
# 201-source libicuuc, 254-source libicui18n, and mmap-backed host data setup.
check_sha256 platform/external/icu "$icu_revision" \
  "$icu_root/Android.bp" \
  07e845ee0ed5660e0e89e64021143856ed9b069ae0a9a97f8acd55a6136f8266
check_sha256 platform/external/icu "$icu_revision" \
  "$icu_root/icu4c/source/common/Android.bp" \
  6a7eb3eb9761f22adc4227af51e4cbd4f8759d26dde6aef50b2cfec5e85638c9
check_sha256 platform/external/icu "$icu_revision" \
  "$icu_root/icu4c/source/i18n/Android.bp" \
  ff906f484c04992bef6b4c1895dc4435f5f65376707bdcc8c7e53014e572996e
check_sha256 platform/external/icu "$icu_revision" \
  "$icu_root/icu4c/source/stubdata/Android.bp" \
  8270c4d784ea9446ed58ce506722f3d8d491ef284826188e285ad16dc44fd064
check_sha256 platform/external/icu "$icu_revision" \
  "$icu_root/libandroidicuinit/Android.bp" \
  98f2a3ed79d1e71a14634b66de73d7e6efbc3f64abd9b3696981e6cf34dad2f8
check_glob_count platform/external/icu "$icu_revision" \
  "$icu_root/icu4c/source/common" .cpp 201
check_glob_count platform/external/icu "$icu_revision" \
  "$icu_root/icu4c/source/i18n" .cpp 254
check_glob_count platform/external/icu "$icu_revision" \
  "$icu_root/icu4c/source/stubdata" .cpp 1
check_glob_count platform/external/icu "$icu_revision" \
  "$icu_root/libandroidicuinit" .cpp 2
check_sha256 platform/external/icu "$icu_revision" \
  "$icu_root/icu4c/source/stubdata/icudt76l.dat" \
  c5450087565eb20ca37d70af5ef53a99a4c8e2e3da17c9140582b685f06d980f
if [[ ! -f "$icu_root/android_icu4c/include/uconfig_local.h" ]]; then
  record_failure "platform/external/icu@$icu_revision: missing $icu_root/android_icu4c/include/uconfig_local.h"
fi

check_sha256 platform/external/freetype "$freetype_revision" \
  "$freetype_root/Android.bp" \
  16983ad80a26a6a0e87232cc226b7a57fb8f08e5968b0ef2207713896a742af7
freetype_sources=(
  src/autofit/autofit.c
  src/base/ftbase.c src/base/ftbbox.c src/base/ftbitmap.c
  src/base/ftdebug.c src/base/ftfstype.c src/base/ftgasp.c
  src/base/ftglyph.c src/base/ftinit.c src/base/ftmm.c src/base/ftstroke.c
  src/base/fttype1.c src/base/ftsystem.c src/cid/type1cid.c src/cff/cff.c
  src/gzip/ftgzip.c src/psaux/psaux.c src/pshinter/pshinter.c
  src/psnames/psnames.c src/raster/raster.c src/sdf/sdf.c src/sfnt/sfnt.c
  src/smooth/smooth.c src/svg/svg.c src/truetype/truetype.c
  src/type1/type1.c
)
check_source_list platform/external/freetype "$freetype_revision" \
  "$freetype_root" "${freetype_sources[@]}"

# These tiny generated Minikin fixtures are preferable to an unpinned system
# font for the first deterministic shaping executable.
check_sha256 platform/frameworks/minikin "$minikin_revision" \
  "$minikin_root/tests/data/Ascii.ttf" \
  3747ed19af40728701dc2c1accc0684fd6c2c72dba08f3f96263269e0846cffe
check_sha256 platform/frameworks/minikin "$minikin_revision" \
  "$minikin_root/tests/data/Arabic.ttf" \
  dce476b160ce641d424a1d03216d6c541ffd768115dbcd665ef0e425e711d5b7
check_sha256 platform/frameworks/minikin "$minikin_revision" \
  "$minikin_root/tests/data/Hangul.ttf" \
  f51078f1915c63440334e2c61290e1461f26b6661151eb6cba3cd81e749fbb9f

# Shaping itself does not require Skia. The immediately following glyph-raster
# acceptance does, and must not silently reuse the current empty font manager.
skia_args="$project_root/_build/skia-text/args.gn"
if [[ ! -f "$skia_args" ]]; then
  record_failure "platform/external/skia@$skia_revision: missing generated GN args $skia_args"
else
  for required_arg in \
    'skia_use_freetype = true' \
    'skia_enable_fontmgr_empty = false' \
    'skia_use_system_freetype2 = true'; do
    if ! grep -Fxq "$required_arg" "$skia_args"; then
      record_failure "platform/external/skia@$skia_revision: $skia_args lacks '$required_arg'"
    fi
  done
  if ! grep -F 'skia_system_freetype2_include_path' "$skia_args" | \
      grep -F "$freetype_root/include" >/dev/null; then
    record_failure "platform/external/skia@$skia_revision: system FreeType include is not pinned to $freetype_root/include"
  fi
fi

if (( ${#failures[@]} != 0 )); then
  echo "text-shaping-inputs: not ready (${#failures[@]} blocker(s))" >&2
  printf '  - %s\n' "${failures[@]}" >&2
  echo "text-shaping-inputs: no fallback ICU, font, FreeType, or symbol shim is permitted" >&2
  exit 2
fi

echo "text-shaping-inputs: minikin=${#minikin_sources[@]} harfbuzz=${#harfbuzz_sources[@]}"
echo "text-shaping-inputs: icu-common=201 icu-i18n=254 icu-stubdata=1 icu-init=2"
echo "text-shaping-inputs: freetype=${#freetype_sources[@]} fonts=3"
echo "text-shaping-inputs: source-and-raster-inputs-ready"
