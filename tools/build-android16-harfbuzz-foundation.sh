#!/bin/bash
set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/.." && pwd)"
source_root="${HARFBUZZ_SOURCE_ROOT:-$project_root/_aosp/external/harfbuzz_ng}"
build_dir="${HARFBUZZ_BUILD_DIR:-$project_root/_build/harfbuzz-foundation}"
object_dir="$build_dir/objects"
lock_file="$project_root/upstream/android16-harfbuzz.lock"
archive_only=0

# Repository-owned assignment-only source identity.
# shellcheck disable=SC1090
source "$lock_file"

usage() {
  echo "usage: $0 [--archive-only]" >&2
  echo "  --archive-only  stop after the complete arm64 libharfbuzz_ng archive gate" >&2
}

case "${1:-}" in
  --archive-only) archive_only=1; shift ;;
  --help|-h) usage; exit 0 ;;
  "") ;;
  *) usage; exit 64 ;;
esac
if [[ $# -ne 0 ]]; then
  usage
  exit 64
fi

HARFBUZZ_SOURCE_ROOT="$source_root" "$script_dir/verify-android16-harfbuzz-source.sh"

android_bp="$source_root/Android.bp"
icu_common="${HARFBUZZ_ICU_COMMON:-$project_root/_aosp/external/icu-graphics/icu4c/source/common}"
required_include_dirs=(
  "$source_root/src"
  "$icu_common"
  "$project_root/_aosp/system/logging/liblog/include"
  "$project_root/_aosp/system/core/libcutils/include"
  "$project_root/_aosp/system/core/libutils/include"
  "$project_root/_aosp/system/core/libutils/binder/include"
  "$project_root/_aosp/system/libbase/include"
)
for include_dir in "${required_include_dirs[@]}"; do
  if [[ ! -d "$include_dir" ]]; then
    echo "harfbuzz-foundation: missing required include subtree: $include_dir" >&2
    echo "HarfBuzz requires module-complete ICU, liblog, libcutils, and libutils headers; no compatibility stubs are permitted." >&2
    echo "HarfBuzz project=platform/external/harfbuzz_ng revision=e489c416b6f8d2a9a2e0e85b781d1e4a0c431401" >&2
    echo "ICU project=platform/external/icu revision=f17caeafcf20bd38074a9963c31df3629b70b5f5" >&2
    exit 2
  fi
done

icu_header_manifest="$(find "$icu_common/unicode" -type f -print \
  | sed "s#^$icu_common/##" \
  | LC_ALL=C sort \
  | while IFS= read -r relative_file; do
      digest="$(shasum -a 256 "$icu_common/$relative_file" | awk '{print $1}')"
      printf '%s  %s\n' "$digest" "$relative_file"
    done)"
icu_header_count="$(printf '%s\n' "$icu_header_manifest" | wc -l | tr -d ' ')"
icu_header_sha="$(printf '%s\n' "$icu_header_manifest" | shasum -a 256 | awk '{print $1}')"
if [[ "$icu_header_count" != "$ICU_PUBLIC_HEADER_FILE_COUNT" ||
      "$icu_header_sha" != "$ICU_PUBLIC_HEADER_MANIFEST_SHA256" ]]; then
  echo "harfbuzz-foundation: ICU public header manifest mismatch" >&2
  echo "expected count=$ICU_PUBLIC_HEADER_FILE_COUNT sha=$ICU_PUBLIC_HEADER_MANIFEST_SHA256" >&2
  echo "actual count=$icu_header_count sha=$icu_header_sha" >&2
  exit 2
fi

extract_module_sources() {
  awk '
    /name:[[:space:]]*"libharfbuzz_ng"/ { in_module = 1 }
    in_module && /srcs:[[:space:]]*\[/ { in_sources = 1; next }
    in_sources && /\],[[:space:]]*$/ { exit }
    in_sources {
      line = $0
      if (match(line, /"[^"]+"/)) {
        print substr(line, RSTART + 1, RLENGTH - 2)
      }
    }
  ' "$android_bp"
}

sources=()
while IFS= read -r source; do
  sources+=("$source")
done < <(extract_module_sources)
if [[ ${#sources[@]} -ne 53 ]]; then
  echo "harfbuzz-foundation: internal source selection changed after source verification" >&2
  exit 3
fi

cxx="${CXX:-$(command -v clang++)}"
if command -v llvm-ar >/dev/null 2>&1; then
  ar="$(command -v llvm-ar)"
else
  ar="$(xcrun --find ar)"
fi
jobs="${JOBS:-$(sysctl -n hw.logicalcpu 2>/dev/null || echo 4)}"
if ! [[ "$jobs" =~ ^[1-9][0-9]*$ ]]; then
  echo "harfbuzz-foundation: JOBS must be a positive integer, got '$jobs'" >&2
  exit 64
fi

compile_flags=(
  -std=gnu++20
  -arch arm64
  -O2
  -fPIC
  -fvisibility=hidden
  -DHAVE_PTHREAD
  -DHB_NO_PRAGMA_GCC_DIAGNOSTIC
  -DHAVE_OT
  -DHAVE_ICU
  -DHAVE_ICU_BUILTIN
  -Werror
  -Wno-unused-parameter
  -Wno-missing-field-initializers
  -Wno-implicit-fallthrough
  -I"$source_root/src"
  -I"$icu_common"
  -I"$project_root/_aosp/system/logging/liblog/include"
  -I"$project_root/_aosp/system/core/libcutils/include"
  -I"$project_root/_aosp/system/core/libutils/include"
  -I"$project_root/_aosp/system/core/libutils/binder/include"
  -I"$project_root/_aosp/system/libbase/include"
)

mkdir -p "$object_dir"
objects=()
batch_pids=()
batch_sources=()
compile_batch() {
  local status=0 index
  for index in "${!batch_pids[@]}"; do
    if ! wait "${batch_pids[$index]}"; then
      echo "harfbuzz-foundation: compile failed: ${batch_sources[$index]}" >&2
      status=1
    fi
  done
  batch_pids=()
  batch_sources=()
  return "$status"
}

for source in "${sources[@]}"; do
  object_name="${source//\//_}"
  object="$object_dir/${object_name%.cc}.o"
  objects+=("$object")
  echo "harfbuzz-foundation: compile $source"
  (
    "$cxx" "${compile_flags[@]}" -c "$source_root/$source" -o "$object"
  ) &
  batch_pids+=("$!")
  batch_sources+=("$source")
  if [[ ${#batch_pids[@]} -ge "$jobs" ]]; then
    compile_batch
  fi
done
if [[ ${#batch_pids[@]} -gt 0 ]]; then
  compile_batch
fi

archive="$build_dir/libharfbuzz_ng-darwin.a"
rm -f "$archive"
"$ar" rcs "$archive" "${objects[@]}"

architectures="$(lipo -archs "$archive")"
if [[ "$architectures" != "arm64" ]]; then
  echo "harfbuzz-foundation: unexpected archive architectures: $architectures" >&2
  exit 3
fi
member_count="$({ "$ar" -t "$archive" || true; } | grep -v '^__\.SYMDEF' | wc -l | tr -d ' ')"
if [[ "$member_count" != "53" ]]; then
  echo "harfbuzz-foundation: archive member count mismatch: expected=53 actual=$member_count" >&2
  exit 3
fi
definitions="$(nm -gU "$archive")"
for symbol in _hb_buffer_create _hb_ot_layout_has_substitution _hb_shape _hb_unicode_funcs_get_default; do
  if ! grep -F " T $symbol" <<<"$definitions" >/dev/null; then
    echo "harfbuzz-foundation: representative definition missing: $symbol" >&2
    exit 3
  fi
done

echo "harfbuzz-foundation: archive=$archive sources=${#sources[@]} members=$member_count arch=$architectures"
if [[ "$archive_only" -eq 1 ]]; then
  echo "harfbuzz-foundation: archive-only gate passed"
  exit 0
fi

liblog_archive="${HARFBUZZ_LIBLOG_ARCHIVE:-$project_root/_build/graphics-foundations/liblog-darwin.a}"
libcutils_archive="${HARFBUZZ_LIBCUTILS_ARCHIVE:-$project_root/_build/graphics-foundations/libcutils-darwin.a}"
libutils_archive="${HARFBUZZ_LIBUTILS_ARCHIVE:-$project_root/_build/graphics-foundations/libutils-darwin.a}"
libicu_archive="${HARFBUZZ_LIBICU_ARCHIVE:-$project_root/_build/icu-foundation/libicu-darwin.a}"
icuuc_archive="${HARFBUZZ_ICUUC_ARCHIVE:-$project_root/_build/icu-foundation/libicuuc-darwin.a}"
icui18n_archive="${HARFBUZZ_ICUI18N_ARCHIVE:-$project_root/_build/icu-foundation/libicui18n-darwin.a}"
dependency_archives=(
  "$libicu_archive"
  "$icui18n_archive"
  "$icuuc_archive"
  "$libutils_archive"
  "$libcutils_archive"
  "$liblog_archive"
)
missing_archives=()
for dependency_archive in "${dependency_archives[@]}"; do
  if [[ ! -f "$dependency_archive" ]]; then
    missing_archives+=("$dependency_archive")
  fi
done
if [[ ${#missing_archives[@]} -ne 0 ]]; then
  echo "harfbuzz-foundation: module archive passed, but executable link gate is blocked" >&2
  echo "missing module-complete dependency archives:" >&2
  for missing_archive in "${missing_archives[@]}"; do
    echo "  $missing_archive" >&2
  done
  echo "required Android.bp dependency closure: libicu, liblog, libcutils, libutils" >&2
  echo "per-symbol stubs and reduced substitute archives are not accepted" >&2
  exit 2
fi

smoke_object="$build_dir/harfbuzz_smoke.o"
smoke_binary="$build_dir/harfbuzz_smoke"
"$cxx" -std=gnu++20 -arch arm64 -O2 -I"$source_root/src" \
  -c "$project_root/probes/harfbuzz_smoke.cc" -o "$smoke_object"
"$cxx" -arch arm64 "$smoke_object" \
  -Wl,-force_load,"$archive" \
  "${dependency_archives[@]}" \
  -o "$smoke_binary"
"$smoke_binary"
echo "harfbuzz-foundation: module-complete executable gate passed: $smoke_binary"
