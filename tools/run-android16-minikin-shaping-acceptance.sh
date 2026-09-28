#!/bin/bash
set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/.." && pwd)"
aosp_root="$project_root/_aosp"
build_dir="$project_root/_build/minikin-shaping-acceptance"
object_dir="$build_dir/objects"

minikin_root="$aosp_root/frameworks/minikin"
harfbuzz_root="$aosp_root/external/harfbuzz_ng"
freetype_root="$aosp_root/external/freetype"
icu_root="$aosp_root/external/icu-graphics"
font_dir="$minikin_root/tests/data"
adapter_dir="$minikin_root/tests/util"

minikin_revision=1e1d5d137d487df875d7db69b5ff24e7d0291612
harfbuzz_revision=e489c416b6f8d2a9a2e0e85b781d1e4a0c431401
freetype_revision=d968d2541f7158e18ab22680bfa08a538019bf6a
icu_revision=f17caeafcf20bd38074a9963c31df3629b70b5f5

verify_lock_value() {
  local lock_file="$1"
  local key="$2"
  local expected="$3"
  local actual
  if [[ ! -f "$lock_file" ]]; then
    echo "minikin-shaping: missing source lock $lock_file" >&2
    exit 2
  fi
  actual="$(awk -F= -v key="$key" '$1 == key { print substr($0, index($0, "=") + 1); exit }' "$lock_file")"
  if [[ "$actual" != "$expected" ]]; then
    echo "minikin-shaping: source lock mismatch $key expected=$expected actual=$actual" >&2
    exit 2
  fi
}

verify_sha256() {
  local file="$1"
  local expected="$2"
  local actual
  if [[ ! -f "$file" ]]; then
    echo "minikin-shaping: missing pinned input $file" >&2
    exit 2
  fi
  actual="$(shasum -a 256 "$file" | awk '{print $1}')"
  if [[ "$actual" != "$expected" ]]; then
    echo "minikin-shaping: checksum mismatch $file" >&2
    echo "  expected=$expected actual=$actual" >&2
    exit 2
  fi
}

verify_revision_marker() {
  local root="$1"
  local expected="$2"
  if [[ -f "$root/.source-revision" ]] &&
      [[ "$(tr -d '[:space:]' < "$root/.source-revision")" != "$expected" ]]; then
    echo "minikin-shaping: source revision marker mismatch $root" >&2
    echo "  expected=$expected" >&2
    exit 2
  fi
}

verify_lock_value "$project_root/upstream/android16-hwui.lock" MINIKIN_REVISION "$minikin_revision"
verify_lock_value "$project_root/upstream/android16-hwui.lock" HARFBUZZ_NG_REVISION "$harfbuzz_revision"
verify_lock_value "$project_root/upstream/android16-hwui.lock" FREETYPE_REVISION "$freetype_revision"
verify_lock_value "$project_root/upstream/android16-hwui.lock" ICU_REVISION "$icu_revision"
verify_revision_marker "$minikin_root" "$minikin_revision"
verify_revision_marker "$harfbuzz_root/src" "$harfbuzz_revision"
verify_revision_marker "$freetype_root" "$freetype_revision"
verify_revision_marker "$icu_root" "$icu_revision"

# These are upstream Minikin test assets, not synthetic substitute fonts. The
# adapter exercises FreeType glyph coverage/metrics/bounds and the Ligature.ttf
# GSUB table makes entry into HarfBuzz observable as a one-glyph `fi` result.
verify_sha256 "$adapter_dir/FreeTypeMinikinFontForTest.h" \
  9028d52b96d9827f9da4ed228592c5ad7de625a934e28a3fa1c856dc4f4e29cb
verify_sha256 "$adapter_dir/FreeTypeMinikinFontForTest.cpp" \
  7cfcc288fc1f084275facbfd307884c9ccda4bb48c0412e01bf8a20b771704a2
verify_sha256 "$font_dir/Ascii.ttf" \
  3747ed19af40728701dc2c1accc0684fd6c2c72dba08f3f96263269e0846cffe
verify_sha256 "$font_dir/Arabic.ttf" \
  dce476b160ce641d424a1d03216d6c541ffd768115dbcd665ef0e425e711d5b7
verify_sha256 "$font_dir/Hangul.ttf" \
  f51078f1915c63440334e2c61290e1461f26b6661151eb6cba3cd81e749fbb9f
verify_sha256 "$font_dir/ControlCharacters.ttf" \
  37aea3915e6a925a445740c486aa0c84fe8e7b07e000b54953ba374cd09d8a62
verify_sha256 "$font_dir/Ligature.ttf" \
  a7b956973d1724fda0b4cc426ca254d1f81b780aa05d13a91feea429ba11a3df

cxx="${CXX:-$(command -v clang++ || true)}"
if [[ -z "$cxx" ]]; then
  echo "minikin-shaping: clang++ is required" >&2
  exit 2
fi
sdk_root="$(xcrun --sdk macosx --show-sdk-path)"
mkdir -p "$object_dir"

compile_flags=(
  -std=c++20
  -arch arm64
  -isysroot "$sdk_root"
  -O2
  -fPIC
  -fno-rtti
  -Wall
  -Wextra
  -Werror
  -Wno-deprecated-declarations
  -Wno-inconsistent-missing-override
  -I"$minikin_root/include"
  -I"$adapter_dir"
  -I"$freetype_root/include"
  -I"$harfbuzz_root/src"
  -I"$icu_root/icu4c/source/common"
  -I"$aosp_root/system/core/libutils/include"
  -I"$aosp_root/system/core/libcutils/include"
  -I"$aosp_root/system/logging/liblog/include"
  -I"$aosp_root/external/googletest/googletest/include"
)

probe_object="$object_dir/minikin_shaping_acceptance.o"
adapter_object="$object_dir/FreeTypeMinikinFontForTest.o"
"$cxx" "${compile_flags[@]}" \
  -c "$project_root/probes/minikin_shaping_acceptance.cc" -o "$probe_object"
"$cxx" "${compile_flags[@]}" \
  -c "$adapter_dir/FreeTypeMinikinFontForTest.cpp" -o "$adapter_object"
for object in "$probe_object" "$adapter_object"; do
  if ! file "$object" | grep -F 'Mach-O 64-bit object arm64' >/dev/null; then
    echo "minikin-shaping: non-arm64 probe object $object" >&2
    exit 3
  fi
done
echo "minikin-shaping: probe and upstream FreeType adapter compiled for arm64"

minikin_archive="${MINIKIN_ARCHIVE:-$project_root/_build/minikin-foundation/libminikin.a}"
harfbuzz_archive="${MINIKIN_HARFBUZZ_ARCHIVE:-$project_root/_build/harfbuzz-foundation/libharfbuzz_ng-darwin.a}"
freetype_archive="${MINIKIN_FREETYPE_ARCHIVE:-$project_root/_build/graphics-codecs/libft2-darwin.a}"
png_archive="${MINIKIN_PNG_ARCHIVE:-$project_root/_build/graphics-codecs/libpng-darwin.a}"
z_archive="${MINIKIN_Z_ARCHIVE:-$project_root/_build/graphics-codecs/libz-darwin.a}"
base_archive="${MINIKIN_BASE_ARCHIVE:-$project_root/_build/foundation/libandroid-base-darwin.a}"
utils_archive="${MINIKIN_UTILS_ARCHIVE:-$project_root/_build/graphics-foundations/libutils-darwin.a}"
cutils_archive="${MINIKIN_CUTILS_ARCHIVE:-$project_root/_build/graphics-foundations/libcutils-darwin.a}"
log_archive="${MINIKIN_LOG_ARCHIVE:-$project_root/_build/graphics-foundations/liblog-darwin.a}"

platform_archives=(
  "$minikin_archive"
  "$harfbuzz_archive"
  "$freetype_archive"
  "$png_archive"
  "$z_archive"
  "$base_archive"
  "$utils_archive"
  "$cutils_archive"
  "$log_archive"
)
missing_platform=0
for archive in "${platform_archives[@]}"; do
  if [[ ! -f "$archive" ]]; then
    echo "minikin-shaping: missing module-complete dependency archive $archive" >&2
    missing_platform=1
  elif [[ "$(lipo -archs "$archive")" != arm64 ]]; then
    echo "minikin-shaping: dependency archive is not arm64-only $archive" >&2
    exit 3
  fi
done
if (( missing_platform != 0 )); then
  echo "minikin-shaping: build producers:" >&2
  echo "  tools/build-android16-minikin-foundation.sh" >&2
  echo "  tools/build-android16-harfbuzz-foundation.sh --archive-only" >&2
  echo "  tools/build-android16-graphics-codecs.sh" >&2
  echo "  tools/build-android16-graphics-foundations.sh" >&2
  echo "  cargo run -p art-bootstrap -- build-foundation" >&2
  exit 2
fi

link_without_icu() {
  "$cxx" -arch arm64 -isysroot "$sdk_root" \
    "$probe_object" "$adapter_object" \
    -Wl,-force_load,"$minikin_archive" \
    -Wl,-force_load,"$harfbuzz_archive" \
    "$freetype_archive" "$png_archive" "$z_archive" "$base_archive" \
    "$utils_archive" "$cutils_archive" "$log_archive" \
    -o "$build_dir/minikin-shaping-without-icu"
}

icu_common_archive="${MINIKIN_ICU_COMMON_ARCHIVE:-$project_root/_build/icu-foundation/libicuuc-common-darwin.a}"
icu_i18n_archive="${MINIKIN_ICU_I18N_ARCHIVE:-$project_root/_build/icu-foundation/libicui18n-darwin.a}"
icu_stubdata_archive="${MINIKIN_ICU_STUBDATA_ARCHIVE:-$project_root/_build/icu-foundation/libicuuc-stubdata-darwin.a}"
icu_init_archive="${MINIKIN_ICU_INIT_ARCHIVE:-$project_root/_build/icu-foundation/libandroidicuinit-darwin.a}"
icu_data="${MINIKIN_ICU_DATA:-$project_root/_build/icu-foundation/runtime/i18n/etc/icu/icudt76l.dat}"
icu_archives=(
  "$icu_i18n_archive"
  "$icu_common_archive"
  "$icu_stubdata_archive"
  "$icu_init_archive"
)

missing_icu=0
for archive in "${icu_archives[@]}"; do
  if [[ ! -f "$archive" ]]; then
    missing_icu=1
  fi
done
if [[ ! -f "$icu_data" ]]; then
  missing_icu=1
fi

if (( missing_icu != 0 )); then
  if link_output="$(link_without_icu 2>&1)"; then
    echo "minikin-shaping: link unexpectedly succeeded without pinned ICU" >&2
    exit 3
  fi
  missing_symbols="$(printf '%s\n' "$link_output" \
    | sed -n -E 's/^  "(_[^"]+)",? referenced from:.*/\1/p' \
    | LC_ALL=C sort -u)"
  expected_icu_symbols=(
    _u_charDirection_76
    _u_charType_76
    _u_cleanup_76
    _u_errorName_76
    _u_getIntPropertyValue_76
    _u_getVersion_76
    _u_hasBinaryProperty_76
    _u_iscntrl_76
    _u_versionToString_76
    _ubidi_close_76
    _ubidi_countRuns_76
    _ubidi_getParaLevel_76
    _ubidi_getVisualRun_76
    _ubidi_open_76
    _ubidi_setClassCallback_76
    _ubidi_setPara_76
    _ubrk_close_76
    _ubrk_following_76
    _ubrk_isBoundary_76
    _ubrk_next_76
    _ubrk_open_76
    _ubrk_setUText_76
    _uloc_addLikelySubtags_76
    _uloc_canonicalize_76
    _uloc_forLanguageTag_76
    _uloc_toLanguageTag_76
    _unorm2_getNFDInstance_76
    _unorm2_getRawDecomposition_76
    _uscript_getScript_76
    _utext_close_76
    _utext_openUChars_76
  )
  expected_symbols="$(printf '%s\n' "${expected_icu_symbols[@]}")"
  if [[ "$missing_symbols" != "$expected_symbols" ]]; then
    diff -u <(printf '%s\n' "$expected_symbols") \
      <(printf '%s\n' "$missing_symbols") >&2 || true
    echo "minikin-shaping: non-ICU or changed ICU undefined-symbol closure detected" >&2
    exit 3
  fi
  echo "minikin-shaping: compile closure passed; executable link is blocked only by 31 ICU 76 symbols" >&2
  echo "minikin-shaping: materialize platform/external/icu@$icu_revision and run:" >&2
  echo "  tools/build-android16-icu-foundation.sh" >&2
  echo "minikin-shaping: required module-complete outputs:" >&2
  for archive in "${icu_archives[@]}"; do
    [[ -f "$archive" ]] || echo "  $archive" >&2
  done
  [[ -f "$icu_data" ]] || echo "  $icu_data" >&2
  echo "minikin-shaping: ICU source closure common=201 i18n=254 stubdata=1 libandroidicuinit=2" >&2
  exit 2
fi

for archive in "${icu_archives[@]}"; do
  if [[ "$(lipo -archs "$archive")" != arm64 ]]; then
    echo "minikin-shaping: ICU archive is not arm64-only $archive" >&2
    exit 3
  fi
done
verify_sha256 "$icu_data" c5450087565eb20ca37d70af5ef53a99a4c8e2e3da17c9140582b685f06d980f

executable="$build_dir/minikin-shaping-acceptance"
"$cxx" -arch arm64 -isysroot "$sdk_root" \
  "$probe_object" "$adapter_object" \
  -Wl,-force_load,"$minikin_archive" \
  -Wl,-force_load,"$harfbuzz_archive" \
  "$freetype_archive" "$png_archive" "$z_archive" "$base_archive" \
  "$utils_archive" "$cutils_archive" "$log_archive" \
  "$icu_i18n_archive" "$icu_common_archive" \
  -Wl,-force_load,"$icu_init_archive" \
  "$icu_stubdata_archive" \
  -o "$executable"
if [[ "$(lipo -archs "$executable")" != arm64 ]]; then
  echo "minikin-shaping: acceptance executable is not arm64-only" >&2
  exit 3
fi
if otool -L "$executable" | grep -E '(/opt/homebrew|/usr/local|lib(icu|harfbuzz|freetype))' >/dev/null; then
  echo "minikin-shaping: executable linked a forbidden host text library" >&2
  otool -L "$executable" >&2
  exit 3
fi

runtime_dir="$build_dir/runtime"
mkdir -p "$runtime_dir/data" "$runtime_dir/tzdata" "$runtime_dir/i18n/etc/icu"
cp "$icu_data" "$runtime_dir/i18n/etc/icu/icudt76l.dat"
output="$(ANDROID_DATA="$runtime_dir/data" \
ANDROID_TZDATA_ROOT="$runtime_dir/tzdata" \
ANDROID_I18N_ROOT="$runtime_dir/i18n" \
  "$executable" "$font_dir")"
printf '%s\n' "$output"

grep -F 'icu-version=76.' <<<"$output" >/dev/null
for case_name in ascii-click arabic-bidi hangul harfbuzz-ligature; do
  grep -E "^case=$case_name .* digest=[0-9a-f]{16}$" <<<"$output" >/dev/null
done
grep -E '^case=harfbuzz-ligature glyphs=1 ' <<<"$output" >/dev/null
grep -F 'minikin-shaping-acceptance: PASS cases=4' <<<"$output" >/dev/null

echo "minikin-shaping: real Minikin/FreeType/HarfBuzz/ICU shaping acceptance passed"
echo "minikin-shaping: executable=$executable"
