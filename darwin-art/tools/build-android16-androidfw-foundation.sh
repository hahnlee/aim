#!/bin/bash
set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/.." && pwd)"
aosp="$project_root/_aosp"
lock_file="$project_root/upstream/android16-androidfw-foundation.lock"
androidfw_root="$aosp/frameworks/base/libs/androidfw"
incfs_root="$aosp/system/incremental_delivery/incfs"
build_dir="$project_root/_build/androidfw-foundation"

# The lock is a trusted assignment-only repository file.
# shellcheck disable=SC1090
source "$lock_file"

fail_missing() {
  local project="$1" revision="$2" path="$3"
  echo "androidfw-foundation: required revision-locked input is missing" >&2
  echo "  project=$project" >&2
  echo "  revision=$revision" >&2
  echo "  path=$path" >&2
  exit 2
}

verify_hash() {
  local project="$1" revision="$2" path="$3" expected="$4"
  [[ -f "$path" ]] || fail_missing "$project" "$revision" "$path"
  local actual
  actual="$(shasum -a 256 "$path" | awk '{print $1}')"
  if [[ "$actual" != "$expected" ]]; then
    echo "androidfw-foundation: revision-locked input checksum mismatch" >&2
    echo "  project=$project" >&2
    echo "  revision=$revision" >&2
    echo "  path=$path" >&2
    echo "  expected=$expected" >&2
    echo "  actual=$actual" >&2
    exit 2
  fi
}

verify_revision_marker() {
  local project="$1" root="$2" expected="$3"
  local marker="$root/.source-revision"
  [[ -f "$marker" ]] || fail_missing "$project" "$expected" "$marker"
  local actual
  actual="$(tr -d '[:space:]' < "$marker")"
  if [[ "$actual" != "$expected" ]]; then
    echo "androidfw-foundation: dependency revision marker mismatch" >&2
    echo "  project=$project" >&2
    echo "  root=$root" >&2
    echo "  expected=$expected" >&2
    echo "  actual=$actual" >&2
    exit 2
  fi
}

verify_hash platform/frameworks/base "$FRAMEWORKS_BASE_REVISION" \
  "$androidfw_root/Android.bp" "$ANDROIDFW_ANDROID_BP_SHA256"

# Complete common srcs list from the checksum-locked libandroidfw Android.bp.
androidfw_sources=(
  ApkAssets.cpp
  ApkParsing.cpp
  Asset.cpp
  AssetDir.cpp
  AssetManager.cpp
  AssetManager2.cpp
  AssetsProvider.cpp
  AttributeResolution.cpp
  BigBuffer.cpp
  BigBufferStream.cpp
  ChunkIterator.cpp
  ConfigDescription.cpp
  FileStream.cpp
  Idmap.cpp
  LoadedArsc.cpp
  Locale.cpp
  LocaleData.cpp
  LocaleDataLookup.cpp
  misc.cpp
  NinePatch.cpp
  ObbFile.cpp
  PosixUtils.cpp
  Png.cpp
  PngChunkFilter.cpp
  PngCrunch.cpp
  ResourceTimer.cpp
  ResourceTypes.cpp
  ResourceUtils.cpp
  StreamingZipInflater.cpp
  StringPool.cpp
  TypeWrappers.cpp
  Util.cpp
  ZipFileRO.cpp
  ZipUtils.cpp
)

if [[ "${#androidfw_sources[@]}" != "$ANDROIDFW_SOURCE_COUNT" ]]; then
  echo "androidfw-foundation: hard-coded Android.bp source count drift" >&2
  exit 3
fi

stage_parent="$build_dir/stage"
mkdir -p "$stage_parent"
stage_dir="$(mktemp -d "$stage_parent/build.XXXXXX")"
cleanup() {
  rm -rf "$stage_dir"
}
trap cleanup EXIT

source_list="$stage_dir/androidfw-sources.txt"
printf '%s\n' "${androidfw_sources[@]}" > "$source_list"
source_list_sha="$(shasum -a 256 "$source_list" | awk '{print $1}')"
if [[ "$source_list_sha" != "$ANDROIDFW_SOURCE_LIST_SHA256" ]]; then
  echo "androidfw-foundation: Android.bp source list changed" >&2
  echo "  expected=$ANDROIDFW_SOURCE_LIST_SHA256" >&2
  echo "  actual=$source_list_sha" >&2
  exit 3
fi

source_manifest="$stage_dir/androidfw-source-manifest.txt"
for source in "${androidfw_sources[@]}"; do
  path="$androidfw_root/$source"
  [[ -f "$path" ]] || fail_missing platform/frameworks/base \
    "$FRAMEWORKS_BASE_REVISION" "$path"
  printf '%s  %s\n' "$(shasum -a 256 "$path" | awk '{print $1}')" "$source" \
    >> "$source_manifest"
done
source_manifest_sha="$(shasum -a 256 "$source_manifest" | awk '{print $1}')"
if [[ "$source_manifest_sha" != "$ANDROIDFW_SOURCE_MANIFEST_SHA256" ]]; then
  echo "androidfw-foundation: Android.bp source content manifest changed" >&2
  echo "  expected=$ANDROIDFW_SOURCE_MANIFEST_SHA256" >&2
  echo "  actual=$source_manifest_sha" >&2
  exit 3
fi

verify_hash platform/frameworks/base "$FRAMEWORKS_BASE_REVISION" \
  "$androidfw_root/PathUtils.cpp" "$ANDROIDFW_PATHUTILS_SHA256"
verify_hash "$INCREMENTAL_DELIVERY_PROJECT" "$INCREMENTAL_DELIVERY_REVISION" \
  "$incfs_root/util/map_ptr.cpp" "$INCFS_MAP_PTR_SHA256"
verify_hash "$INCREMENTAL_DELIVERY_PROJECT" "$INCREMENTAL_DELIVERY_REVISION" \
  "$incfs_root/util/include/util/map_ptr.h" "$INCFS_MAP_PTR_HEADER_SHA256"

frameworks_native="$aosp/frameworks/native"
system_core="$aosp/system/core"
system_logging="$aosp/system/logging"
libbase="$aosp/system/libbase"
libziparchive="$aosp/system/libziparchive"
fmtlib="$aosp/external/fmtlib"
libpng="$aosp/external/libpng"
zlib="$aosp/external/zlib"

verify_hash platform/frameworks/native "$FRAMEWORKS_NATIVE_REVISION" \
  "$frameworks_native/include/ftl/static_vector.h" \
  "$FRAMEWORKS_NATIVE_FTL_STATIC_VECTOR_SHA256"
verify_revision_marker platform/system/core "$system_core/libutils" \
  "$SYSTEM_CORE_REVISION"
verify_revision_marker platform/system/core "$system_core/libcutils" \
  "$SYSTEM_CORE_REVISION"
verify_hash platform/system/core "$SYSTEM_CORE_REVISION" \
  "$system_core/libutils/include/utils/String8.h" \
  "$SYSTEM_CORE_STRING8_HEADER_SHA256"
verify_revision_marker platform/system/logging "$system_logging/liblog" \
  "$SYSTEM_LOGGING_REVISION"
verify_revision_marker platform/system/libbase "$libbase" "$LIBBASE_REVISION"
verify_hash platform/system/libbase "$LIBBASE_REVISION" \
  "$libbase/include/android-base/result.h" "$LIBBASE_RESULT_HEADER_SHA256"
verify_revision_marker platform/system/libziparchive "$libziparchive" \
  "$LIBZIPARCHIVE_REVISION"
verify_hash platform/system/libziparchive "$LIBZIPARCHIVE_REVISION" \
  "$libziparchive/include/ziparchive/zip_archive.h" \
  "$LIBZIPARCHIVE_PUBLIC_HEADER_SHA256"
verify_hash platform/external/fmtlib "$FMTLIB_REVISION" \
  "$fmtlib/include/fmt/format.h" "$FMTLIB_FORMAT_HEADER_SHA256"
verify_revision_marker platform/external/libpng "$libpng" "$LIBPNG_REVISION"
verify_revision_marker platform/external/zlib "$zlib" "$ZLIB_REVISION"

required_archives=(
  "$project_root/_build/foundation/libandroid-base-darwin.a"
  "$project_root/_build/foundation/libziparchive-darwin.a"
  "$project_root/_build/graphics-foundations/libutils-darwin.a"
  "$project_root/_build/graphics-foundations/libutils-binder-darwin.a"
  "$project_root/_build/graphics-foundations/libcutils-darwin.a"
  "$project_root/_build/graphics-foundations/liblog-darwin.a"
  "$project_root/_build/graphics-codecs/libpng-darwin.a"
  "$project_root/_build/graphics-codecs/libz-darwin.a"
)
for archive in "${required_archives[@]}"; do
  [[ -f "$archive" ]] || fail_missing darwin-art-foundation local "$archive"
done

cxx="$(command -v clang++ || true)"
[[ -n "$cxx" ]] || { echo "androidfw-foundation: clang++ is required" >&2; exit 2; }
libtool_bin="$(xcrun --find libtool)"
ld_bin="$(xcrun --find ld)"
sdk_root="$(xcrun --sdk macosx --show-sdk-path)"
sdk_version="$(xcrun --sdk macosx --show-sdk-version)"
object_dir="$stage_dir/objects"
mkdir -p "$object_dir"

common_flags=(
  -std=gnu++23 -arch arm64 -O2 -fPIC -fno-rtti -fvisibility=hidden
  # Soong global cflags: ALOGV/LOG_NDEBUG tracing and debug asserts are off.
  -DNDEBUG -UDEBUG
  -Wall -Werror -Wunreachable-code -DSTATIC_ANDROIDFW_FOR_TOOLS
  # Apple Clang diagnoses unchanged Android 16 source/libc++ constructs that
  # Android's pinned host toolchain accepts. Keep these compatibility flags
  # local to this gate; none changes the Android C++ ABI.
  -Wno-nontrivial-memcall
  -Wno-deprecated-declarations
  -Wno-vla-cxx-extension
  -Wno-deprecated-anon-enum-enum-conversion
  -Wno-unused-variable
  -I"$androidfw_root/include"
  -I"$androidfw_root/include_pathutils"
  -I"$frameworks_native/include"
  -I"$libbase/include"
  -I"$fmtlib/include"
  -I"$system_core/libutils/include"
  -I"$system_core/libutils/binder/include"
  -I"$system_core/libcutils/include"
  -I"$system_core/libsystem/include"
  -I"$system_logging/liblog/include"
  -I"$libziparchive/include"
  -I"$incfs_root/util/include"
  -I"$libpng"
  -I"$zlib"
)

objects=()
for source in "${androidfw_sources[@]}"; do
  object="$object_dir/${source%.cpp}.o"
  echo "androidfw-foundation: compile $source"
  "$cxx" "${common_flags[@]}" -c "$androidfw_root/$source" -o "$object"
  objects+=("$object")
done

pathutils_object="$object_dir/PathUtils.o"
echo "androidfw-foundation: compile PathUtils.cpp (whole-static)"
"$cxx" "${common_flags[@]}" -c "$androidfw_root/PathUtils.cpp" \
  -o "$pathutils_object"
objects+=("$pathutils_object")

incfs_object="$object_dir/incfs_map_ptr.o"
echo "androidfw-foundation: compile libincfs-utils/util/map_ptr.cpp (whole-static)"
"$cxx" -std=gnu++20 -arch arm64 -O2 -fPIC -fno-rtti \
  -Wall -Wextra -Werror -Wno-deprecated-enum-enum-conversion \
  -D_FILE_OFFSET_BITS=64 \
  -I"$incfs_root/util/include" -I"$libbase/include" -I"$fmtlib/include" \
  -I"$system_core/libutils/include" -I"$system_core/libutils/binder/include" \
  -I"$system_logging/liblog/include" \
  -c "$incfs_root/util/map_ptr.cpp" -o "$incfs_object"
objects+=("$incfs_object")

for object in "${objects[@]}"; do
  if [[ "$(file "$object")" != *"Mach-O 64-bit object arm64"* ]]; then
    echo "androidfw-foundation: non-arm64 object produced: $object" >&2
    exit 3
  fi
done

archive="$stage_dir/libandroidfw-darwin.a"
"$libtool_bin" -static -o "$archive" "${objects[@]}"
member_count="$({ ar -t "$archive" || true; } | grep -v '^__\.SYMDEF' | wc -l | tr -d ' ')"
if [[ "$member_count" != "$ARCHIVE_MEMBER_COUNT" ]]; then
  echo "androidfw-foundation: archive member count mismatch" >&2
  echo "  expected=$ARCHIVE_MEMBER_COUNT actual=$member_count" >&2
  exit 3
fi

combined="$stage_dir/androidfw-force-loaded.o"
"$ld_bin" -r -arch arm64 -platform_version macos "$sdk_version" "$sdk_version" \
  -syslibroot "$sdk_root" -force_load "$archive" -o "$combined"
if [[ "$(file "$combined")" != *"Mach-O 64-bit object arm64"* ]]; then
  echo "androidfw-foundation: force-loaded object is not arm64 Mach-O" >&2
  exit 3
fi

definitions="$stage_dir/androidfw-definitions.txt"
# Inspect archive members before ld -r internalizes hidden-visibility symbols.
nm -gU "$archive" | c++filt > "$definitions"
for symbol in \
  'android::AssetManager2::SetApkAssets(' \
  'android::ConfigDescription::Parse(' \
  'android::ResTable_config::compare(' \
  'android::ZipFileRO::open(' \
  'android::getPathLeaf(' \
  'android::incfs::IncFsFileMap::Create('; do
  if ! grep -F "$symbol" "$definitions" >/dev/null; then
    echo "androidfw-foundation: representative definition missing: $symbol" >&2
    exit 3
  fi
done

undefined_manifest="$stage_dir/androidfw-undefined-symbols.txt"
nm -u "$combined" | awk '$1 ~ /^_/ { print $1 }' | LC_ALL=C sort -u \
  > "$undefined_manifest"
undefined_count="$(wc -l < "$undefined_manifest" | tr -d ' ')"
undefined_sha="$(shasum -a 256 "$undefined_manifest" | awk '{print $1}')"
if [[ -n "$UNDEFINED_SYMBOL_COUNT" && "$undefined_count" != "$UNDEFINED_SYMBOL_COUNT" ]]; then
  echo "androidfw-foundation: undefined-symbol count drift" >&2
  echo "  expected=$UNDEFINED_SYMBOL_COUNT actual=$undefined_count" >&2
  exit 3
fi
if [[ -n "$UNDEFINED_SYMBOLS_SHA256" && "$undefined_sha" != "$UNDEFINED_SYMBOLS_SHA256" ]]; then
  echo "androidfw-foundation: undefined-symbol manifest drift" >&2
  echo "  expected=$UNDEFINED_SYMBOLS_SHA256 actual=$undefined_sha" >&2
  exit 3
fi

probe_object="$stage_dir/android16_androidfw_smoke.o"
probe_executable="$stage_dir/android16-androidfw-smoke"
"$cxx" "${common_flags[@]}" \
  -c "$project_root/probes/android16_androidfw_smoke.cpp" -o "$probe_object"
"$cxx" -arch arm64 "$probe_object" "$archive" \
  "$project_root/_build/graphics-foundations/libutils-darwin.a" \
  "$project_root/_build/graphics-foundations/libutils-binder-darwin.a" \
  "$project_root/_build/graphics-foundations/libcutils-darwin.a" \
  "$project_root/_build/graphics-foundations/liblog-darwin.a" \
  "$project_root/_build/foundation/libandroid-base-darwin.a" \
  "$project_root/_build/foundation/libziparchive-darwin.a" \
  "$project_root/_build/graphics-codecs/libpng-darwin.a" \
  "$project_root/_build/graphics-codecs/libz-darwin.a" \
  -L/opt/homebrew/lib -lfmt -o "$probe_executable"

set +e
probe_output="$("$probe_executable")"
probe_status=$?
set -e
expected_probe='androidfw host archive: BigBuffer+PathUtils+IncFsFileMap ok'
if [[ "$probe_status" != 0 || "$probe_output" != "$expected_probe" ]]; then
  echo "androidfw-foundation: smoke failed with status $probe_status" >&2
  echo "androidfw-foundation: unexpected smoke output: $probe_output" >&2
  exit 4
fi

mkdir -p "$build_dir"
cp "$archive" "$build_dir/libandroidfw-darwin.a"
cp "$combined" "$build_dir/androidfw-force-loaded.o"
cp "$source_list" "$build_dir/androidfw-sources.txt"
cp "$source_manifest" "$build_dir/androidfw-source-manifest.txt"
cp "$undefined_manifest" "$build_dir/androidfw-undefined-symbols.txt"
cp "$probe_executable" "$build_dir/android16-androidfw-smoke"

echo "androidfw-foundation: sources=${#androidfw_sources[@]} whole-static=2 archive-members=$member_count"
echo "androidfw-foundation: undefined=$undefined_count sha256=$undefined_sha"
echo "$probe_output"
