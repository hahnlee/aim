#!/bin/bash
set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/.." && pwd)"
lock_file="$project_root/upstream/android16-icu-jni.lock"
source_root="$project_root/_aosp/external/icu-jni"
destination="$source_root/android_icu4j/libcore_bridge/src/native"
download_dir="$project_root/_downloads"

# shellcheck disable=SC1090
source "$lock_file"

fail_gate() {
  echo "icu-jni-materialize: $1" >&2
  exit 2
}

manifest_sha() {
  local root="$1"
  shift
  local file hash
  for file in "$@"; do
    hash="$(shasum -a 256 "$root/$file" | awk '{print $1}')"
    printf '%s  %s\n' "$hash" "$file"
  done | shasum -a 256 | awk '{print $1}'
}

verify_tree() {
  local root="$1"
  [[ -f "$root/Android.bp" ]] || fail_gate "missing $root/Android.bp"

  local bp_sha
  bp_sha="$(shasum -a 256 "$root/Android.bp" | awk '{print $1}')"
  [[ "$bp_sha" == "$ANDROID_BP_SHA256" ]] ||
    fail_gate "Android.bp checksum mismatch expected=$ANDROID_BP_SHA256 actual=$bp_sha"

  local cpp_sources=() headers=() tree_files=()
  while IFS= read -r file; do cpp_sources+=("${file##*/}"); done < <(
    find "$root" -maxdepth 1 -type f -name '*.cpp' | sort
  )
  while IFS= read -r file; do headers+=("${file##*/}"); done < <(
    find "$root" -maxdepth 1 -type f -name '*.h' | sort
  )

  [[ "${#cpp_sources[@]}" == "$CPP_SOURCE_COUNT" ]] ||
    fail_gate "C++ source count mismatch expected=$CPP_SOURCE_COUNT actual=${#cpp_sources[@]}"
  [[ "${#headers[@]}" == "$HEADER_SOURCE_COUNT" ]] ||
    fail_gate "header count mismatch expected=$HEADER_SOURCE_COUNT actual=${#headers[@]}"
  [[ "$(manifest_sha "$root" "${cpp_sources[@]}")" == "$CPP_SOURCE_MANIFEST_SHA256" ]] ||
    fail_gate "C++ source manifest mismatch"
  [[ "$(manifest_sha "$root" "${headers[@]}")" == "$HEADER_SOURCE_MANIFEST_SHA256" ]] ||
    fail_gate "header manifest mismatch"

  tree_files=(Android.bp "${cpp_sources[@]}" "${headers[@]}")
  [[ "$(manifest_sha "$root" "${tree_files[@]}")" == "$SOURCE_TREE_MANIFEST_SHA256" ]] ||
    fail_gate "source tree manifest mismatch"
}

if [[ -d "$destination" ]]; then
  [[ ! -e "$source_root/.git" ]] || fail_gate "Git metadata is forbidden: $source_root/.git"
  [[ -f "$source_root/.source-revision" ]] || fail_gate "missing source revision marker"
  [[ "$(tr -d '[:space:]' < "$source_root/.source-revision")" == "$ICU_REVISION" ]] ||
    fail_gate "source revision marker mismatch"
  verify_tree "$destination"
  echo "icu-jni-materialize: verified revision=$ICU_REVISION source=$destination"
  exit 0
fi
[[ ! -e "$source_root" ]] ||
  fail_gate "partial source root exists without the locked subtree: $source_root"

mkdir -p "$download_dir" "$(dirname "$source_root")"
archive="$download_dir/external-icu-jni-$ICU_REVISION.tar.gz"
if [[ ! -f "$archive" ]]; then
  partial="$archive.partial"
  curl -fL --retry 3 \
    "https://android.googlesource.com/$ICU_PROJECT/+archive/$ICU_REVISION/$ICU_JNI_SUBTREE.tar.gz" \
    -o "$partial"
  mv "$partial" "$archive"
fi

stage="$(mktemp -d "$(dirname "$source_root")/icu-jni.stage.XXXXXX")"
trap 'rm -rf "$stage"' EXIT
stage_destination="$stage/android_icu4j/libcore_bridge/src/native"
mkdir -p "$stage_destination"
tar -xzf "$archive" -C "$stage_destination"
verify_tree "$stage_destination"
printf '%s\n' "$ICU_REVISION" > "$stage/.source-revision"
mv "$stage" "$source_root"
trap - EXIT

echo "icu-jni-materialize: revision=$ICU_REVISION source=$destination"
