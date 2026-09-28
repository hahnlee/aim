#!/bin/bash
set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/.." && pwd)"
lock_file="$project_root/upstream/android16-icu-jni.lock"
foundation_lock="$project_root/upstream/android16-icu-foundation.lock"
source_root="$project_root/_aosp/external/icu-jni"
source_dir="$source_root/android_icu4j/libcore_bridge/src/native"
icu_root="$project_root/_aosp/external/icu-graphics"
build_dir="$project_root/_build/icu-jni-foundation"

# shellcheck disable=SC1090
source "$lock_file"

fail_gate() {
  echo "icu-jni-foundation: $1" >&2
  exit 2
}

verify_sha() {
  local file="$1" expected="$2" actual
  [[ -f "$file" ]] || fail_gate "missing pinned input $file"
  actual="$(shasum -a 256 "$file" | awk '{print $1}')"
  [[ "$actual" == "$expected" ]] ||
    fail_gate "checksum mismatch file=$file expected=$expected actual=$actual"
}

[[ -f "$source_root/.source-revision" ]] ||
  fail_gate "source is absent; run tools/materialize-android16-icu-jni.sh"
[[ "$(tr -d '[:space:]' < "$source_root/.source-revision")" == "$ICU_REVISION" ]] ||
  fail_gate "source revision mismatch"
grep -Fx "ICU_REVISION=$ICU_REVISION" "$foundation_lock" >/dev/null ||
  fail_gate "ICU foundation revision mismatch"
[[ -f "$source_dir/Android.bp" ]] || fail_gate "missing pinned Android.bp"
[[ "$(shasum -a 256 "$source_dir/Android.bp" | awk '{print $1}')" == "$ANDROID_BP_SHA256" ]] ||
  fail_gate "Android.bp checksum mismatch"
verify_sha "$project_root/compat/icu-jni/byteswap.h" "$BYTESWAP_COMPAT_SHA256"
verify_sha "$project_root/compat/icu-jni/stdatomic.h" "$STDATOMIC_COMPAT_SHA256"
verify_sha "$project_root/include/aim/icu_jni.h" "$PUBLIC_HEADER_SHA256"

manifest_sha() {
  local root="$1"
  shift
  local file hash
  for file in "$@"; do
    hash="$(shasum -a 256 "$root/$file" | awk '{print $1}')"
    printf '%s  %s\n' "$hash" "$file"
  done | shasum -a 256 | awk '{print $1}'
}

sources=()
headers=()
while IFS= read -r file; do sources+=("${file##*/}"); done < <(
  find "$source_dir" -maxdepth 1 -type f -name '*.cpp' | sort
)
while IFS= read -r file; do headers+=("${file##*/}"); done < <(
  find "$source_dir" -maxdepth 1 -type f -name '*.h' | sort
)
[[ "${#sources[@]}" == "$CPP_SOURCE_COUNT" ]] ||
  fail_gate "Android.bp source count mismatch expected=$CPP_SOURCE_COUNT actual=${#sources[@]}"
[[ "${#headers[@]}" == "$HEADER_SOURCE_COUNT" ]] ||
  fail_gate "header count mismatch expected=$HEADER_SOURCE_COUNT actual=${#headers[@]}"
[[ "$(manifest_sha "$source_dir" "${sources[@]}")" == "$CPP_SOURCE_MANIFEST_SHA256" ]] ||
  fail_gate "Android.bp source manifest mismatch"
[[ "$(manifest_sha "$source_dir" "${headers[@]}")" == "$HEADER_SOURCE_MANIFEST_SHA256" ]] ||
  fail_gate "header manifest mismatch"
tree_files=(Android.bp "${sources[@]}" "${headers[@]}")
[[ "$(manifest_sha "$source_dir" "${tree_files[@]}")" == "$SOURCE_TREE_MANIFEST_SHA256" ]] ||
  fail_gate "source tree manifest mismatch"

cxx="$(command -v clang++ || true)"
[[ -n "$cxx" ]] || fail_gate "clang++ is required"
ar="$(command -v llvm-ar || xcrun --find ar)"
sdk_root="$(xcrun --sdk macosx --show-sdk-path)"

mkdir -p "$build_dir"
stage="$(mktemp -d "$build_dir/stage.XXXXXX")"
trap 'rm -rf "$stage"' EXIT
object_dir="$stage/objects"
mkdir -p "$object_dir"

flags=(
  -arch arm64
  -isysroot "$sdk_root"
  -std=gnu++20
  -O2
  -fPIC
  -fvisibility=hidden
  -Wall
  -Wextra
  -Werror
  -Wno-unused-parameter
  -Wno-deprecated-declarations
  -Wno-unnecessary-virtual-specifier
  -DANDROID
  -DU_USING_ICU_NAMESPACE=0
  -I"$project_root/compat/icu-jni"
  -I"$source_dir"
  -I"$project_root/_aosp/libnativehelper-full/include"
  -I"$project_root/_aosp/libnativehelper-full/include_jni"
  -I"$project_root/_aosp/libnativehelper-full/header_only_include"
  -I"$project_root/_aosp/libnativehelper-full/include_platform"
  -I"$project_root/_aosp/libnativehelper-full/include_platform_header_only"
  -I"$project_root/_aosp/system/libbase/include"
  -I"$project_root/_aosp/system/logging/liblog/include"
  -I"$project_root/_aosp/system/core/libcutils/include"
  -I"$icu_root/android_icu4c/include"
  -I"$icu_root/icu4c/source/common"
  -I"$icu_root/icu4c/source/i18n"
)

objects=()
for source in "${sources[@]}"; do
  object="$object_dir/${source%.cpp}.o"
  if [[ "$source" == Register.cpp ]]; then
    source_flags=(
      "-DJNI_OnLoad=$SAFE_JNI_ONLOAD_SYMBOL"
      "-DJNI_OnUnload=$SAFE_JNI_ONUNLOAD_SYMBOL"
    )
    echo "icu-jni-foundation: compile $source"
    "$cxx" "${flags[@]}" "${source_flags[@]}" -c "$source_dir/$source" -o "$object"
  else
    echo "icu-jni-foundation: compile $source"
    "$cxx" "${flags[@]}" -c "$source_dir/$source" -o "$object"
  fi
  file "$object" | grep -F 'Mach-O 64-bit object arm64' >/dev/null ||
    fail_gate "object is not Mach-O arm64: $object"
  objects+=("$object")
done

archive="$stage/libicu-jni-darwin.a"
"$ar" rcs "$archive" "${objects[@]}"
[[ "$(lipo -archs "$archive")" == arm64 ]] || fail_gate "archive is not arm64"
members="$("$ar" -t "$archive" | grep -v '^__\.SYMDEF' | wc -l | tr -d ' ')"
[[ "$members" == "$ARCHIVE_MEMBER_COUNT" ]] ||
  fail_gate "archive member count mismatch expected=$ARCHIVE_MEMBER_COUNT actual=$members"

definitions="$(nm -gU "$archive")"
for symbol in "_$SAFE_JNI_ONLOAD_SYMBOL" "_$SAFE_JNI_ONUNLOAD_SYMBOL"; do
  grep -E "[[:space:]]T $symbol$" <<<"$definitions" >/dev/null ||
    fail_gate "archive lacks required definition $symbol"
done
for symbol in \
  register_com_android_icu_util_regex_PatternNative \
  register_com_android_icu_util_regex_MatcherNative \
  register_com_android_icu_util_charset_NativeConverter; do
  grep -F "$symbol" <<<"$definitions" >/dev/null ||
    fail_gate "archive lacks required registration definition $symbol"
done
if grep -E '[[:space:]]T _JNI_On(Load|Unload)$' <<<"$definitions" >/dev/null; then
  fail_gate "unscoped JNI_OnLoad/JNI_OnUnload leaked from static module"
fi

force_loaded="$stage/libicu-jni-force-loaded.o"
"$cxx" -r -arch arm64 -isysroot "$sdk_root" \
  -Wl,-force_load,"$archive" -o "$force_loaded"
[[ "$(lipo -archs "$force_loaded")" == arm64 ]] ||
  fail_gate "force-loaded object is not arm64"

mv "$archive" "$build_dir/libicu-jni-darwin.a"
mv "$force_loaded" "$build_dir/libicu-jni-force-loaded.o"
"$ar" -t "$build_dir/libicu-jni-darwin.a" | grep -v '^__\.SYMDEF' \
  > "$build_dir/archive-members.txt"
nm -gU "$build_dir/libicu-jni-darwin.a" | \
  awk '$2 ~ /^[TDBSCRGWV]$/ && $3 ~ /^_/ { print $3 }' | sort -u \
  > "$build_dir/global-definitions.txt"
nm -u "$build_dir/libicu-jni-force-loaded.o" | awk '$1 ~ /^_/ { print $1 }' | sort -u \
  > "$build_dir/undefined-symbols.txt"

member_sha="$(shasum -a 256 "$build_dir/archive-members.txt" | awk '{print $1}')"
[[ "$member_sha" == "$ARCHIVE_MEMBER_MANIFEST_SHA256" ]] ||
  fail_gate "archive member manifest mismatch"
definition_count="$(wc -l < "$build_dir/global-definitions.txt" | tr -d ' ')"
definition_sha="$(shasum -a 256 "$build_dir/global-definitions.txt" | awk '{print $1}')"
[[ "$definition_count" == "$GLOBAL_DEFINITION_COUNT" &&
   "$definition_sha" == "$GLOBAL_DEFINITION_MANIFEST_SHA256" ]] ||
  fail_gate "global definition identity mismatch count=$definition_count sha=$definition_sha"
undefined_count="$(wc -l < "$build_dir/undefined-symbols.txt" | tr -d ' ')"
undefined_sha="$(shasum -a 256 "$build_dir/undefined-symbols.txt" | awk '{print $1}')"
[[ "$undefined_count" == "$UNDEFINED_SYMBOL_COUNT" &&
   "$undefined_sha" == "$UNDEFINED_SYMBOL_MANIFEST_SHA256" ]] ||
  fail_gate "undefined symbol identity mismatch count=$undefined_count sha=$undefined_sha"

echo "icu-jni-foundation: revision=$ICU_REVISION objects=${#objects[@]}"
echo "icu-jni-foundation: safe-entry=$SAFE_JNI_ONLOAD_SYMBOL"
echo "icu-jni-foundation: definitions=$definition_count unresolved=$undefined_count"
echo "icu-jni-foundation: archive=$build_dir/libicu-jni-darwin.a"
