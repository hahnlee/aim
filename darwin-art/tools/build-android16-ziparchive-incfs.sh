#!/bin/bash
set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/.." && pwd)"
source_root="$project_root/_aosp/system/libziparchive"
lock_file="$project_root/upstream/android16-ziparchive-incfs.lock"
output_dir="$project_root/_build/ziparchive-incfs"

# shellcheck disable=SC1090
source "$lock_file"

fail() {
  echo "ziparchive-for-incfs: $1" >&2
  exit 2
}

sha256() { shasum -a 256 "$1" | awk '{print $1}'; }
[[ -f "$source_root/Android.bp" ]] || fail "missing source; run art-bootstrap sync"
[[ "$(sha256 "$source_root/Android.bp")" == "$ANDROID_BP_SHA256" ]] ||
  fail "Android.bp identity mismatch"
libbase_strerror_sha="$(sha256 "$project_root/_aosp/system/libbase/posix_strerror_r.cpp")"
[[ "$libbase_strerror_sha" == "$LIBBASE_POSIX_STRERROR_R_SHA256" ]] ||
  fail "libbase strerror identity mismatch"

sources=(
  zip_archive.cc
  zip_archive_stream_entry.cc
  zip_cd_entry_map.cc
  zip_error.cpp
  zip_writer.cc
  incfs_support/signal_handling.cpp
)
[[ "${#sources[@]}" == "$SOURCE_COUNT" ]] || fail "internal source count mismatch"
source_list="$(printf '%s\n' "${sources[@]}")"
source_list_sha="$(printf '%s\n' "$source_list" | shasum -a 256 | awk '{print $1}')"
[[ "$source_list_sha" == "$SOURCE_LIST_SHA256" ]] || fail "source list mismatch"
manifest="$({
  for source in "${sources[@]}"; do
    [[ -f "$source_root/$source" ]] || fail "missing source $source"
    printf '%s  %s\n' "$(sha256 "$source_root/$source")" "$source"
  done
})"
manifest_sha="$(printf '%s\n' "$manifest" | shasum -a 256 | awk '{print $1}')"
[[ "$manifest_sha" == "$SOURCE_MANIFEST_SHA256" ]] || fail "source manifest mismatch"

cxx="$(xcrun --find clang++)"
ar="$(xcrun --find ar)"
sdk="$(xcrun --sdk macosx --show-sdk-path)"
stage="$(mktemp -d "$output_dir.stage.XXXXXX")"
trap 'rm -rf "$stage"' EXIT
mkdir -p "$stage/objects"
mkdir -p "$stage/source"
archive_open_patch="$project_root/patches/ziparchive/0001-darwin-archive-open.patch"
[[ "$(sha256 "$archive_open_patch")" == "$ARCHIVE_OPEN_PATCH_SHA256" ]] || fail "archive open patch mismatch"
cp "$source_root/zip_archive.cc" "$stage/source/zip_archive.cc"
(cd "$stage/source" && patch --batch --forward -p1 -i "$archive_open_patch")

flags=(
  -arch arm64 -isysroot "$sdk" -std=c++20 -O2 -fPIC
  -Wall -Werror -Wno-missing-field-initializers -Wconversion
  -Wno-sign-conversion -Wold-style-cast
  -DZLIB_CONST -D_FILE_OFFSET_BITS=64 -DZIPARCHIVE_DISABLE_CALLBACK_API=1
  -I"$source_root" -I"$source_root/include"
  -include "$project_root/compat/filesystem/archive_open.h"
  -I"$source_root/incfs_support/include"
  -I"$project_root/_aosp/system/libbase/include"
  -I"$project_root/_aosp/system/logging/liblog/include"
  -I"$project_root/_aosp/external/googletest/googletest/include"
)

objects=()
for source in "${sources[@]}"; do
  object="$stage/objects/${source//\//_}.o"
  echo "ziparchive-for-incfs: compile $source"
  input="$source_root/$source"
  [[ "$source" != zip_archive.cc ]] || input="$stage/source/zip_archive.cc"
  "$cxx" "${flags[@]}" -c "$input" -o "$object"
  file "$object" | grep -F 'Mach-O 64-bit object arm64' >/dev/null
  objects+=("$object")
done

archive="$stage/libziparchive-for-incfs-darwin.a"
"$ar" rcs "$archive" "${objects[@]}"
members="$("$ar" -t "$archive" | grep -v '^__\.SYMDEF' | wc -l | tr -d ' ')"
[[ "$members" == "$ARCHIVE_MEMBER_COUNT" ]] || fail "archive member count=$members"
lipo -info "$archive" | grep -F 'architecture: arm64' >/dev/null

force_loaded="$stage/ziparchive-for-incfs-force-loaded.o"
"$cxx" -r -arch arm64 -isysroot "$sdk" -Wl,-force_load,"$archive" \
  -o "$force_loaded"
nm -u "$force_loaded" | awk '$1 ~ /^_/ { print $1 }' | sort -u > "$stage/undefined.txt"
undefined_count="$(wc -l < "$stage/undefined.txt" | tr -d ' ')"
undefined_sha="$(sha256 "$stage/undefined.txt")"
[[ "$undefined_count" == "$UNDEFINED_SYMBOL_COUNT" &&
   "$undefined_sha" == "$UNDEFINED_SYMBOLS_SHA256" ]] ||
  fail "undefined closure mismatch count=$undefined_count sha256=$undefined_sha"

definitions="$(nm -gUC "$archive")"
for symbol in 'OpenArchive(char const*' 'ZipWriter::StartEntry(' 'ExtractToMemory('; do
  grep -F "$symbol" <<<"$definitions" >/dev/null || fail "missing definition $symbol"
done

smoke="$stage/android16-ziparchive-incfs-smoke"
"$cxx" -arch arm64 -isysroot "$sdk" -std=c++20 -Wall -Werror \
  -I"$source_root/include" -I"$project_root/_aosp/system/libbase/include" \
  -I"$project_root/_aosp/external/googletest/googletest/include" \
  "$project_root/probes/android16_ziparchive_incfs_smoke.cpp" "$archive" \
  "$project_root/compat/filesystem/archive_open.cc" \
  "$project_root/_aosp/system/libbase/posix_strerror_r.cpp" \
  "$project_root/_build/foundation/libandroid-base-darwin.a" \
  "$project_root/_build/graphics-foundations/liblog-darwin.a" -lz -o "$smoke"
output="$("$smoke")"
[[ "$output" == 'ziparchive-for-incfs: writer+reader payload=darwin-art' ]] ||
  fail "unexpected smoke output: $output"

mkdir -p "$output_dir"
mv "$archive" "$output_dir/libziparchive-for-incfs-darwin.a"
mv "$force_loaded" "$output_dir/ziparchive-for-incfs-force-loaded.o"
mv "$stage/undefined.txt" "$output_dir/undefined-symbols.txt"
mv "$smoke" "$output_dir/android16-ziparchive-incfs-smoke"
echo "ziparchive-for-incfs: members=$members undefined=$undefined_count"
echo "$output"
