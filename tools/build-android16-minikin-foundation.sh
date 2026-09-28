#!/bin/bash
set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/.." && pwd)"
aosp_root="$project_root/_aosp"
lock_file="$project_root/upstream/android16-hwui.lock"
output_dir="$project_root/_build/minikin-foundation"
object_dir="$output_dir/objects"

lock_value() {
  local key="$1"
  awk -F= -v key="$key" \
    '$1 == key { print substr($0, index($0, "=") + 1); exit }' "$lock_file"
}

verify_file() {
  local project="$1"
  local revision="$2"
  local file="$3"
  local expected="$4"
  if [[ ! -f "$file" ]]; then
    echo "minikin-foundation: missing revision-locked input" >&2
    echo "  project:  $project" >&2
    echo "  revision: $revision" >&2
    echo "  file:     $file" >&2
    echo "  sha256:   $expected" >&2
    exit 2
  fi
  local actual
  actual="$(shasum -a 256 "$file" | awk '{print $1}')"
  if [[ "$actual" != "$expected" ]]; then
    echo "minikin-foundation: checksum mismatch" >&2
    echo "  project:  $project" >&2
    echo "  revision: $revision" >&2
    echo "  file:     $file" >&2
    echo "  expected: $expected" >&2
    echo "  actual:   $actual" >&2
    exit 2
  fi
}

minikin_revision="1e1d5d137d487df875d7db69b5ff24e7d0291612"
harfbuzz_revision="e489c416b6f8d2a9a2e0e85b781d1e4a0c431401"
icu_revision="f17caeafcf20bd38074a9963c31df3629b70b5f5"
if [[ "$(lock_value MINIKIN_REVISION)" != "$minikin_revision" ||
      "$(lock_value HARFBUZZ_NG_REVISION)" != "$harfbuzz_revision" ||
      "$(lock_value ICU_REVISION)" != "$icu_revision" ]]; then
  echo "minikin-foundation: Android 16 source lock mismatch" >&2
  exit 2
fi

minikin_root="$aosp_root/frameworks/minikin"
module_root="$minikin_root/libs/minikin"
harfbuzz_root="$aosp_root/external/harfbuzz_ng"
icu_common="$aosp_root/external/icu-graphics/icu4c/source/common"

verify_file platform/frameworks/minikin "$minikin_revision" \
  "$minikin_root/Android.bp" \
  93fc02b83bdf1ab57335d88d4be8d30937a422784e16691b2c812917a5982cb2
verify_file platform/frameworks/minikin "$minikin_revision" \
  "$module_root/Android.bp" \
  8b90ecd388ff80df2dc9cb5bc62421d1e8da68191fb2e4316f1a9ed96f340a01
verify_file platform/external/harfbuzz_ng "$harfbuzz_revision" \
  "$harfbuzz_root/Android.bp" \
  02393c97deee24420fee50100c9beccd5a998bade71b260d170fcdac9ae831f2

# Complete libminikin.srcs from the checksum-locked Android.bp. The optional
# debuggable-only Debug.cpp is deliberately absent from the Darwin release
# variant built by this gate.
sources=(
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
for source in "${sources[@]}"; do
  if [[ ! -f "$module_root/$source" ]]; then
    echo "minikin-foundation: missing module source $module_root/$source" >&2
    echo "project=platform/frameworks/minikin revision=$minikin_revision" >&2
    exit 2
  fi
done
source_manifest="$(for source in "${sources[@]}"; do
  digest="$(shasum -a 256 "$module_root/$source" | awk '{print $1}')"
  printf '%s  %s\n' "$digest" "$source"
done)"
source_manifest_sha="$(printf '%s\n' "$source_manifest" | shasum -a 256 | awk '{print $1}')"
if [[ "$source_manifest_sha" != ba9ff20b96a43c106dc7cc8149044f2ca41776a7538f340c3bc58b6121babebc ]]; then
  echo "minikin-foundation: libminikin source manifest mismatch" >&2
  echo "expected=ba9ff20b96a43c106dc7cc8149044f2ca41776a7538f340c3bc58b6121babebc actual=$source_manifest_sha" >&2
  exit 2
fi

required_include_dirs=(
  "$minikin_root/include"
  "$harfbuzz_root/src"
  "$icu_common"
  "$aosp_root/system/libbase/include"
  "$aosp_root/system/core/libutils/include"
  "$aosp_root/system/core/libcutils/include"
  "$aosp_root/system/core/libsystem/include"
  "$aosp_root/system/logging/liblog/include"
  "$aosp_root/external/googletest/googletest/include"
)
for include_dir in "${required_include_dirs[@]}"; do
  if [[ ! -d "$include_dir" ]]; then
    echo "minikin-foundation: missing exported include tree $include_dir" >&2
    exit 2
  fi
done
for header in unicode/ubidi.h unicode/ubrk.h unicode/uchar.h unicode/unorm2.h; do
  if [[ ! -f "$icu_common/$header" ]]; then
    echo "minikin-foundation: incomplete ICU public headers: $icu_common/$header" >&2
    echo "project=platform/external/icu revision=$icu_revision" >&2
    exit 2
  fi
done

cxx="$(command -v clang++ || true)"
if [[ -z "$cxx" ]]; then
  echo "minikin-foundation: clang++ is required" >&2
  exit 2
fi
libtool_bin="$(xcrun --find libtool)"

common_flags=(
  -std=c++20
  -arch arm64
  -O2
  -fPIC
  -fno-rtti
  -fvisibility=hidden
  -Wall
  -Werror
  -Wextra
  -Wthread-safety
  '-DLOG_TAG="Minikin"'
  # Apple Clang/libc++ diagnostics introduced after Android's pinned host
  # toolchain. Upstream source is unchanged and the Android.bp warnings remain
  # enabled; only these known cross-toolchain diagnostics are suppressed.
  -Wno-deprecated-declarations
  -Wno-unused-variable
  -Wno-inconsistent-missing-override
  -Wno-return-type
  -I"$minikin_root/include"
  -I"$harfbuzz_root/src"
  -I"$icu_common"
  -I"$aosp_root/system/libbase/include"
  -I"$aosp_root/system/core/libutils/include"
  -I"$aosp_root/system/core/libcutils/include"
  -I"$aosp_root/system/core/libsystem/include"
  -I"$aosp_root/system/logging/liblog/include"
  -I"$aosp_root/external/googletest/googletest/include"
)

# Do not add module_root with -I: it contains AOSP's private `locale.h`, which
# would shadow the macOS SDK's <locale.h> while libc++ headers are parsed.
# Quoted private headers resolve relative to each source without that search
# path, matching their intended use.
mkdir -p "$object_dir"
objects=()
for source in "${sources[@]}"; do
  object="$object_dir/${source%.cpp}.o"
  echo "minikin-foundation: compile $source"
  "$cxx" "${common_flags[@]}" -c "$module_root/$source" -o "$object"
  objects+=("$object")
done

archive="$output_dir/libminikin.a"
"$libtool_bin" -static -o "$archive" "${objects[@]}"

for object in "${objects[@]}"; do
  file "$object" | grep -F 'Mach-O 64-bit object arm64' >/dev/null
done
file "$archive" | grep -F 'current ar archive' >/dev/null
lipo -info "$archive" | grep -F 'architecture: arm64' >/dev/null

expected_members="$(printf '%s\n' "${sources[@]%.cpp}" | sed 's/$/.o/' | sort)"
actual_members="$(ar -t "$archive" | grep -v '^__.SYMDEF' | sort)"
if [[ "$actual_members" != "$expected_members" ]]; then
  echo "minikin-foundation: archive members do not match libminikin.srcs" >&2
  diff -u <(printf '%s\n' "$expected_members") \
    <(printf '%s\n' "$actual_members") >&2 || true
  exit 3
fi

definitions="$(nm -gUC "$archive")"
for definition in \
  'minikin::Font::Builder::build()' \
  'minikin::FontCollection::create(' \
  'minikin::Layout::doLayout('; do
  if ! grep -F " T $definition" <<<"$definitions" >/dev/null; then
    echo "minikin-foundation: missing representative definition $definition" >&2
    exit 3
  fi
done

undefined_count="$(nm -u "$archive" | awk '$1 ~ /^_/ { print $1 }' | sort -u \
  | wc -l | tr -d ' ')"

echo "minikin-foundation: objects=${#objects[@]} architecture=arm64"
echo "minikin-foundation: undefined-static-closure=$undefined_count"
echo "minikin-foundation: archive=$archive"
