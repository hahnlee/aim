#!/bin/bash
# AOSP idmap2 (cmds/idmap2) as a Darwin host tool. Image assembly runs its
# `create-multiple` command to write the idmaps of the image's immutable
# framework overlays (ADR 0009); the device path of every APK it reads
# resolves under DARWIN_ART_IDMAP2_GUEST_ROOT (tools/idmap2-host).
set -euo pipefail
export LC_ALL=C

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/.." && pwd)"
lock="$project_root/upstream/android16-idmap2.lock"
revision="$(sed -n 's/^# FRAMEWORKS_BASE_REVISION=//p' "$lock")"
source_root="$project_root/_aosp/frameworks-base-idmap2/cmds/idmap2"
build_dir="$project_root/_build/android16-idmap2"
aosp="$project_root/_aosp"
protobuf="${DARWIN_ART_PROTOBUF_PREFIX:-/opt/homebrew/opt/protobuf}"
abseil="${DARWIN_ART_ABSEIL_PREFIX:-/opt/homebrew/opt/abseil}"

sha256() { shasum -a 256 "$1" | awk '{print $1}'; }
fail() { echo "idmap2: $*" >&2; exit 3; }

while read -r expected relative; do
  [[ -z "$expected" || "$expected" == \#* ]] && continue
  destination="$source_root/$relative"
  if [[ ! -f "$destination" ]]; then
    mkdir -p "$(dirname "$destination")"
    staged="$(mktemp "${destination}.download.XXXXXX")"
    curl -fsSL --retry 3 \
      "https://android.googlesource.com/platform/frameworks/base/+/$revision/cmds/idmap2/$relative?format=TEXT" \
      | base64 -D > "$staged"
    [[ "$(sha256 "$staged")" == "$expected" ]] || fail "download checksum mismatch: $relative"
    mv "$staged" "$destination"
  fi
  [[ "$(sha256 "$destination")" == "$expected" ]] || fail "checksum mismatch: $relative"
done < "$lock"

# The Soong owner of the sources compiled below.
for unit in '"libidmap2/**/*.cpp"' '"self_targeting/*.cpp"' '"idmap2/CreateMultiple.cpp"' \
    '"libidmap2/proto/*.proto"'; do
  grep -F "$unit" "$source_root/Android.bp" >/dev/null || fail "Soong owner changed: $unit"
done
[[ -x "$protobuf/bin/protoc" ]] || fail "protoc is required: $protobuf"

libraries=(
  "$project_root/_build/androidfw-foundation/libandroidfw-darwin.a"
  "$project_root/_build/graphics-foundations/libutils-darwin.a"
  "$project_root/_build/graphics-foundations/libutils-binder-darwin.a"
  "$project_root/_build/graphics-foundations/libcutils-darwin.a"
  "$project_root/_build/graphics-foundations/liblog-darwin.a"
  "$project_root/_build/foundation/libandroid-base-darwin.a"
  "$project_root/_build/foundation/libziparchive-darwin.a"
  "$project_root/_build/graphics-codecs/libpng-darwin.a"
  "$project_root/_build/graphics-codecs/libz-darwin.a"
)
for library in "${libraries[@]}"; do [[ -f "$library" ]] || fail "missing $library"; done

stage="$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-idmap2.XXXXXX")"
trap 'rm -rf "$stage"' EXIT
mkdir -p "$stage/gen" "$stage/objects"
"$protobuf/bin/protoc" --cpp_out="$stage/gen" -I"$source_root" libidmap2/proto/fabricated_v1.proto

cxx="$(xcrun --find clang++)"
sdk_root="$(xcrun --sdk macosx --show-sdk-path)"
flags=(
  -std=gnu++23 -arch arm64 -O2 -DNDEBUG -DSTATIC_ANDROIDFW_FOR_TOOLS -isysroot "$sdk_root"
  -Wno-deprecated-declarations -Wno-vla-cxx-extension -Wno-nontrivial-memcall
  -Wno-deprecated-anon-enum-enum-conversion
  -I"$source_root" -I"$source_root/include" -I"$source_root/libidmap2_policies/include"
  -I"$stage/gen"
  -I"$aosp/frameworks/base/libs/androidfw/include"
  -I"$aosp/frameworks/native/include"
  -I"$aosp/hwui-static-deps/frameworks-native/libs/binder/include"
  -I"$aosp/system/libbase/include" -I"$aosp/external/fmtlib/include"
  -I"$aosp/system/core/libutils/include" -I"$aosp/system/core/libutils/binder/include"
  -I"$aosp/system/core/libcutils/include" -I"$aosp/system/core/libsystem/include"
  -I"$aosp/system/logging/liblog/include" -I"$aosp/system/libziparchive/include"
  -I"$aosp/system/incremental_delivery/incfs/util/include"
  -I"$aosp/external/libpng" -I"$aosp/external/zlib"
  -I"$protobuf/include" -I"$abseil/include"
)
objects=()
compile() {
  local source="$1" object="$stage/objects/$2.o"
  "$cxx" "${flags[@]}" "${@:3}" -c "$source" -o "$object"
  objects+=("$object")
}
for source in "$source_root"/libidmap2/*.cpp "$source_root"/self_targeting/*.cpp \
    "$source_root"/idmap2/{CommandUtils,Create,CreateMultiple,Dump,Lookup,Main}.cpp; do
  compile "$source" "$(basename "$(dirname "$source")")-$(basename "${source%.cpp}")"
done
compile "$stage/gen/libidmap2/proto/fabricated_v1.pb.cc" fabricated_v1
# libbase and fmt members the linked foundation archives do not carry.
compile "$aosp/system/libbase/errors_unix.cpp" libbase-errors_unix
compile "$aosp/system/libbase/posix_strerror_r.cpp" libbase-posix_strerror_r
compile "$aosp/external/fmtlib/src/format.cc" fmt-format
compile "$project_root/tools/idmap2-host/guest_root.cc" guest_root -I"$project_root/compat/filesystem"

mkdir -p "$build_dir"
"$cxx" -arch arm64 -isysroot "$sdk_root" "${objects[@]}" "${libraries[@]}" \
  -L"$protobuf/lib" -lprotobuf-lite -L"$abseil/lib" \
  -labsl_log_internal_check_op -labsl_log_internal_message -labsl_log_internal_nullguard \
  -labsl_log_internal_conditions -labsl_raw_logging_internal -labsl_strings -labsl_hash \
  -labsl_raw_hash_set -labsl_base -labsl_spinlock_wait -labsl_throw_delegate \
  -labsl_status -labsl_statusor -labsl_cord \
  -o "$stage/idmap2" 2> >(grep -v '^ld: warning' >&2)
usage="$(DARWIN_ART_IDMAP2_GUEST_ROOT=/ "$stage/idmap2" 2>&1 || true)"
[[ "$usage" == "usage: idmap2 [create|create-multiple|dump|lookup]" ]] ||
  fail "unexpected idmap2 output: $usage"
mv "$stage/idmap2" "$build_dir/idmap2"
echo "idmap2: $build_dir/idmap2"
