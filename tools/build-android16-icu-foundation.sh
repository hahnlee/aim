#!/bin/bash
set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/.." && pwd)"
lock_file="$project_root/upstream/android16-icu-foundation.lock"
icu_root="${AIM_ANDROID16_ICU_ROOT:-$project_root/_aosp/external/icu-graphics}"
build_dir="$project_root/_build/icu-foundation"

# The lock is a trusted, assignment-only repository file.
# shellcheck disable=SC1090
source "$lock_file"

required_files=(
  Android.bp
  android_icu4c/Android.bp
  icu4c/source/Android.bp
  icu4c/source/common/Android.bp
  icu4c/source/i18n/Android.bp
  icu4c/source/stubdata/Android.bp
  libandroidicuinit/Android.bp
  android_icu4c/include/uconfig_local.h
  "icu4c/source/stubdata/$ICU_DATA_FILE"
)
required_hashes=(
  "$ROOT_ANDROID_BP_SHA256"
  "$ANDROID_ICU4C_ANDROID_BP_SHA256"
  "$SOURCE_ANDROID_BP_SHA256"
  "$COMMON_ANDROID_BP_SHA256"
  "$I18N_ANDROID_BP_SHA256"
  "$STUBDATA_ANDROID_BP_SHA256"
  "$ANDROID_ICU_INIT_ANDROID_BP_SHA256"
  "$UCONFIG_LOCAL_SHA256"
  "$ICU_DATA_SHA256"
)

missing=0
for ((index = 0; index < ${#required_files[@]}; ++index)); do
  relative="${required_files[$index]}"
  if [[ ! -f "$icu_root/$relative" ]]; then
    echo "icu-foundation: missing source=$icu_root/$relative" >&2
    echo "  project=$ICU_PROJECT revision=$ICU_REVISION sha256=${required_hashes[$index]}" >&2
    missing=1
  fi
done
if (( missing != 0 )); then
  echo "icu-foundation: expected complete gitless source; see upstream/android16-icu-foundation.md" >&2
  exit 2
fi
if [[ -e "$icu_root/.git" ]]; then
  echo "icu-foundation: Git metadata is forbidden in $icu_root" >&2
  exit 2
fi
if [[ -f "$icu_root/.source-revision" ]] &&
    [[ "$(tr -d '[:space:]' < "$icu_root/.source-revision")" != "$ICU_REVISION" ]]; then
  echo "icu-foundation: revision marker mismatch in $icu_root" >&2
  exit 2
fi

verify_sha256() {
  local relative="$1"
  local expected="$2"
  local actual
  actual="$(shasum -a 256 "$icu_root/$relative" | awk '{print $1}')"
  if [[ "$actual" != "$expected" ]]; then
    echo "icu-foundation: checksum mismatch $icu_root/$relative" >&2
    echo "  project=$ICU_PROJECT revision=$ICU_REVISION expected=$expected actual=$actual" >&2
    exit 2
  fi
}
verify_sha256 Android.bp "$ROOT_ANDROID_BP_SHA256"
verify_sha256 android_icu4c/Android.bp "$ANDROID_ICU4C_ANDROID_BP_SHA256"
verify_sha256 icu4c/source/Android.bp "$SOURCE_ANDROID_BP_SHA256"
verify_sha256 icu4c/source/common/Android.bp "$COMMON_ANDROID_BP_SHA256"
verify_sha256 icu4c/source/i18n/Android.bp "$I18N_ANDROID_BP_SHA256"
verify_sha256 icu4c/source/stubdata/Android.bp "$STUBDATA_ANDROID_BP_SHA256"
verify_sha256 libandroidicuinit/Android.bp "$ANDROID_ICU_INIT_ANDROID_BP_SHA256"
verify_sha256 android_icu4c/include/uconfig_local.h "$UCONFIG_LOCAL_SHA256"
verify_sha256 "icu4c/source/stubdata/$ICU_DATA_FILE" "$ICU_DATA_SHA256"

actual_data_size="$(stat -f '%z' "$icu_root/icu4c/source/stubdata/$ICU_DATA_FILE")"
if [[ "$actual_data_size" != "$ICU_DATA_SIZE" ]]; then
  echo "icu-foundation: ICU data size mismatch expected=$ICU_DATA_SIZE actual=$actual_data_size" >&2
  exit 2
fi

common_dir="$icu_root/icu4c/source/common"
i18n_dir="$icu_root/icu4c/source/i18n"
stubdata_dir="$icu_root/icu4c/source/stubdata"
init_dir="$icu_root/libandroidicuinit"

common_sources=()
while IFS= read -r file; do common_sources+=("${file##*/}"); done < <(
  find "$common_dir" -maxdepth 1 -type f -name '*.cpp' | sort
)
i18n_sources=()
while IFS= read -r file; do i18n_sources+=("${file##*/}"); done < <(
  find "$i18n_dir" -maxdepth 1 -type f -name '*.cpp' | sort
)
stubdata_sources=(stubdata.cpp)
init_sources=(IcuRegistration.cpp android_icu_init.cpp)

source_manifest_sha256() {
  local root="$1"
  shift
  local source hash
  for source in "$@"; do
    if [[ ! -f "$root/$source" ]]; then
      echo "icu-foundation: missing Android.bp-selected source $root/$source" >&2
      exit 2
    fi
    hash="$(shasum -a 256 "$root/$source" | awk '{print $1}')"
    printf '%s  %s\n' "$hash" "$source"
  done | shasum -a 256 | awk '{print $1}'
}

verify_source_manifest() {
  local module="$1"
  local root="$2"
  local expected_count="$3"
  local expected_sha="$4"
  shift 4
  local actual_sha
  if [[ "$#" != "$expected_count" ]]; then
    echo "icu-foundation: $module source count expected=$expected_count actual=$#" >&2
    exit 2
  fi
  actual_sha="$(source_manifest_sha256 "$root" "$@")"
  if [[ "$actual_sha" != "$expected_sha" ]]; then
    echo "icu-foundation: $module source manifest mismatch" >&2
    echo "  project=$ICU_PROJECT revision=$ICU_REVISION expected=$expected_sha actual=$actual_sha" >&2
    exit 2
  fi
}
verify_source_manifest libicuuc "$common_dir" "$COMMON_SOURCE_COUNT" \
  "$COMMON_SOURCE_MANIFEST_SHA256" "${common_sources[@]}"
verify_source_manifest libicui18n "$i18n_dir" "$I18N_SOURCE_COUNT" \
  "$I18N_SOURCE_MANIFEST_SHA256" "${i18n_sources[@]}"
verify_source_manifest libicuuc_stubdata "$stubdata_dir" "$STUBDATA_SOURCE_COUNT" \
  "$STUBDATA_SOURCE_MANIFEST_SHA256" "${stubdata_sources[@]}"
verify_source_manifest libandroidicuinit "$init_dir" "$ANDROID_ICU_INIT_SOURCE_COUNT" \
  "$ANDROID_ICU_INIT_SOURCE_MANIFEST_SHA256" "${init_sources[@]}"

cxx="$(xcrun --find clang++)"
sdk_root="$(xcrun --sdk macosx --show-sdk-path)"
if command -v llvm-ar >/dev/null 2>&1; then
  ar="$(command -v llvm-ar)"
else
  ar="$(xcrun --find ar)"
fi

mkdir -p "$build_dir"
stage_dir="$(mktemp -d "$build_dir/stage.XXXXXX")"
cleanup() {
  rm -rf "$stage_dir"
}
trap cleanup EXIT
object_dir="$build_dir/objects"
mkdir -p "$object_dir/common" "$object_dir/i18n" \
  "$object_dir/stubdata" "$object_dir/androidicuinit"

compile_cached() {
  local label="$1" source="$2" object="$3"
  shift 3
  local meta="${object}.cmd"
  local command_file="${object}.command"
  local source_sha
  source_sha="$(shasum -a 256 "$source" | awk '{print $1}')"
  local -a command=("$cxx" "$@" -MMD -MF "$object.d" -c "$source" -o "$object")
  local command_text
  command_text="$(printf '%q ' "${command[@]}")"
  local key
  key="$(printf '%s\n%s\n' "$source_sha" "$command_text" | shasum -a 256 | awk '{print $1}')"
  if [[ -f "$object" && -f "$object.d" && -f "$meta" && -f "$command_file" &&
        "$(<"$meta")" == "$key" ]]; then
    echo "icu-foundation: cache $label"
    return
  fi
  echo "icu-foundation: compile $label"
  "${command[@]}"
  printf '%s\n' "$key" > "$meta"
  printf '%s\n' "$command_text" > "${command_file}.tmp.$$"
  mv "${command_file}.tmp.$$" "$command_file"
}

base_flags=(
  -arch arm64
  -isysroot "$sdk_root"
  -std=gnu++20
  -frtti
  -fPIC
  -Wall
  -Werror
  -Wno-ambiguous-reversed-operator
  -Wno-deprecated-declarations
  -Wno-unused-parameter
  -Wno-unused-const-variable
  -Wno-unneeded-internal-declaration
  -DUCONFIG_USE_ML_PHRASE_BREAKING=1
  -DANDROID
  -I"$icu_root/android_icu4c/include"
  -I"$common_dir"
  -I"$i18n_dir"
  -I"$init_dir/include"
)

common_objects=()
for source in "${common_sources[@]}"; do
  object="$object_dir/common/${source%.cpp}.o"
  compile_cached "common/$source" "$common_dir/$source" "$object" \
    "${base_flags[@]}" \
    -D_REENTRANT -DU_COMMON_IMPLEMENTATION -O3 -fvisibility=hidden \
    -Wno-missing-field-initializers -Wno-sign-compare
  common_objects+=("$object")
done

i18n_objects=()
for source in "${i18n_sources[@]}"; do
  object="$object_dir/i18n/${source%.cpp}.o"
  compile_cached "i18n/$source" "$i18n_dir/$source" "$object" \
    "${base_flags[@]}" \
    -D_REENTRANT -DU_I18N_IMPLEMENTATION -O3 -fvisibility=hidden \
    -Wno-unreachable-code-loop-increment
  i18n_objects+=("$object")
done

stubdata_objects=()
for source in "${stubdata_sources[@]}"; do
  object="$object_dir/stubdata/${source%.cpp}.o"
  compile_cached "stubdata/$source" "$stubdata_dir/$source" "$object" \
    -arch arm64 -isysroot "$sdk_root" -std=gnu++20 -fPIC -Wall -Werror \
    -DANDROID -I"$icu_root/android_icu4c/include" -I"$common_dir"
  stubdata_objects+=("$object")
done

init_objects=()
for source in "${init_sources[@]}"; do
  object="$object_dir/androidicuinit/${source%.cpp}.o"
  compile_cached "libandroidicuinit/$source" "$init_dir/$source" "$object" \
    -arch arm64 -isysroot "$sdk_root" -std=gnu++20 -fPIC -Wall -Werror \
    -DANDROID -I"$icu_root/android_icu4c/include" -I"$common_dir" \
    -I"$init_dir" -I"$init_dir/include"
  init_objects+=("$object")
done

common_archive="$stage_dir/libicuuc-common-darwin.a"
i18n_archive="$stage_dir/libicui18n-darwin.a"
stubdata_archive="$stage_dir/libicuuc-stubdata-darwin.a"
init_archive="$stage_dir/libandroidicuinit-darwin.a"
"$ar" rcs "$common_archive" "${common_objects[@]}"
"$ar" rcs "$i18n_archive" "${i18n_objects[@]}"
"$ar" rcs "$stubdata_archive" "${stubdata_objects[@]}"
"$ar" rcs "$init_archive" "${init_objects[@]}"

archive_members() {
  "$ar" -t "$1" | grep -v '^__\.SYMDEF'
}
verify_archive() {
  local archive="$1"
  local expected_count="$2"
  local actual_count
  if [[ "$(lipo -archs "$archive")" != arm64 ]]; then
    echo "icu-foundation: non-arm64 archive $archive" >&2
    exit 3
  fi
  actual_count="$(archive_members "$archive" | wc -l | tr -d ' ')"
  if [[ "$actual_count" != "$expected_count" ]]; then
    echo "icu-foundation: archive member count $archive expected=$expected_count actual=$actual_count" >&2
    exit 3
  fi
}
verify_archive "$common_archive" "$COMMON_SOURCE_COUNT"
verify_archive "$i18n_archive" "$I18N_SOURCE_COUNT"
verify_archive "$stubdata_archive" "$STUBDATA_SOURCE_COUNT"
verify_archive "$init_archive" "$ANDROID_ICU_INIT_SOURCE_COUNT"

runtime_dir="$stage_dir/runtime"
mkdir -p "$runtime_dir/i18n/etc/icu"
cp "$icu_root/icu4c/source/stubdata/$ICU_DATA_FILE" \
  "$runtime_dir/i18n/etc/icu/$ICU_DATA_FILE"
runtime_data_sha="$(shasum -a 256 "$runtime_dir/i18n/etc/icu/$ICU_DATA_FILE" | awk '{print $1}')"
if [[ "$runtime_data_sha" != "$ICU_DATA_SHA256" ]]; then
  echo "icu-foundation: staged data checksum mismatch" >&2
  exit 3
fi

smoke_object="$stage_dir/android16_icu_smoke.o"
smoke_executable="$stage_dir/android16-icu-smoke"
"$cxx" -arch arm64 -isysroot "$sdk_root" -std=gnu++20 -DANDROID \
  -I"$icu_root/android_icu4c/include" -I"$common_dir" -I"$i18n_dir" \
  -I"$init_dir/include" \
  -c "$project_root/probes/android16_icu_smoke.cpp" -o "$smoke_object"
"$cxx" -arch arm64 -isysroot "$sdk_root" "$smoke_object" "$i18n_archive" "$common_archive" \
  -Wl,-force_load,"$init_archive" "$stubdata_archive" \
  -o "$smoke_executable"

if [[ "$(lipo -archs "$smoke_executable")" != arm64 ]]; then
  echo "icu-foundation: smoke executable is not arm64" >&2
  exit 3
fi
linked_libraries="$(otool -L "$smoke_executable")"
if grep -E '(/opt/homebrew|/usr/local|libicu(uc|i18n))' \
    <<<"$linked_libraries" >/dev/null; then
  echo "icu-foundation: smoke executable has forbidden ICU dependency" >&2
  echo "$linked_libraries" >&2
  exit 3
fi
definitions="$(nm -gU "$smoke_executable")"
for symbol in __Z16android_icu_initv __Z20android_icu_registerv \
    _icudt76_dat _u_getVersion_76; do
  if ! grep -F "$symbol" <<<"$definitions" >/dev/null; then
    echo "icu-foundation: linked definition missing $symbol" >&2
    exit 3
  fi
done
if ! nm -gU "$smoke_executable" | c++filt | \
    grep -F 'icu_76::NumberFormat::createInstance' >/dev/null; then
  echo "icu-foundation: linked i18n NumberFormat definition missing" >&2
  exit 3
fi

smoke_output="$(
  ANDROID_DATA="$runtime_dir/data" \
  ANDROID_TZDATA_ROOT="$runtime_dir/tzdata" \
  ANDROID_I18N_ROOT="$runtime_dir/i18n" \
    "$smoke_executable"
)"
printf '%s\n' "$smoke_output"
if ! grep -F 'icu_version=76.' <<<"$smoke_output" >/dev/null; then
  echo "icu-foundation: ICU 76 runtime assertion missing" >&2
  exit 4
fi

for output in libicuuc-common-darwin.a libicui18n-darwin.a \
    libicuuc-stubdata-darwin.a libandroidicuinit-darwin.a \
    android16-icu-smoke; do
  mv "$stage_dir/$output" "$build_dir/$output"
done
rm -rf "$build_dir/runtime"
mv "$runtime_dir" "$build_dir/runtime"

echo "icu-foundation: common=${#common_objects[@]} i18n=${#i18n_objects[@]} stubdata=${#stubdata_objects[@]} init=${#init_objects[@]}"
echo "icu-foundation: arm64 executable=$build_dir/android16-icu-smoke"
