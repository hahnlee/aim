#!/bin/bash
set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/.." && pwd)"
aosp_root="$project_root/_aosp"
lock_file="$project_root/upstream/android16-hostgraphics.lock"
output_dir="$project_root/_build/hostgraphics"
default_source_root="$aosp_root/frameworks/base/libs/hostgraphics"
default_deps_root="$aosp_root/hostgraphics-deps"

# shellcheck disable=SC1090
source "$lock_file"

if [[ -z "${AIM_ANDROID16_HOSTGRAPHICS_ROOT:-}" ||
      -z "${AIM_ANDROID16_NATIVEDISPLAY_INCLUDE:-}" ||
      -z "${AIM_ANDROID16_LIBHARDWARE_INCLUDE:-}" ]]; then
  "$script_dir/sync-android16-hostgraphics.sh"
fi

fail_build() {
  echo "hostgraphics-build: $1" >&2
  exit 2
}

verify_sha() {
  local file="$1" expected="$2" actual
  [[ -f "$file" ]] || fail_build "missing source $file"
  actual="$(shasum -a 256 "$file" | awk '{print $1}')"
  [[ "$actual" == "$expected" ]] ||
    fail_build "checksum mismatch file=$file expected=$expected actual=$actual"
}

source_root="${AIM_ANDROID16_HOSTGRAPHICS_ROOT:-$default_source_root}"
nativedisplay_include="${AIM_ANDROID16_NATIVEDISPLAY_INCLUDE:-$default_deps_root/frameworks-native/libs/nativedisplay/include}"
libhardware_include="${AIM_ANDROID16_LIBHARDWARE_INCLUDE:-$default_deps_root/libhardware/include_all}"

sources=(
  ADisplay.cpp
  ANativeWindow.cpp
  Fence.cpp
  HostBufferQueue.cpp
  PublicFormat.cpp
)

headers=(
  include/gui/BufferItem.h
  include/gui/BufferItemConsumer.h
  include/gui/BufferQueue.h
  include/gui/ConsumerBase.h
  include/gui/IGraphicBufferConsumer.h
  include/gui/IGraphicBufferProducer.h
  include/gui/Surface.h
  include/ui/Fence.h
  include/ui/GraphicBuffer.h
)

verify_sha "$source_root/Android.bp" "$HOSTGRAPHICS_ANDROID_BP_SHA256"
verify_sha "$nativedisplay_include/apex/display.h" "$NATIVEDISPLAY_DISPLAY_H_SHA256"
verify_sha "$libhardware_include/hardware/hardware.h" "$LIBHARDWARE_HARDWARE_H_SHA256"

source_manifest="$({
  for file in "${sources[@]}"; do
    [[ -f "$source_root/$file" ]] || fail_build "missing module source $source_root/$file"
    printf '%s  %s\n' "$(shasum -a 256 "$source_root/$file" | awk '{print $1}')" "$file"
  done
})"
source_manifest_sha="$(printf '%s\n' "$source_manifest" | shasum -a 256 | awk '{print $1}')"
[[ "${#sources[@]}" == "$HOSTGRAPHICS_SOURCE_COUNT" &&
   "$source_manifest_sha" == "$HOSTGRAPHICS_SOURCE_MANIFEST_SHA256" ]] ||
  fail_build "source manifest mismatch expected=$HOSTGRAPHICS_SOURCE_MANIFEST_SHA256 actual=$source_manifest_sha"

header_manifest="$({
  for file in "${headers[@]}"; do
    [[ -f "$source_root/$file" ]] || fail_build "missing module header $source_root/$file"
    printf '%s  %s\n' "$(shasum -a 256 "$source_root/$file" | awk '{print $1}')" "$file"
  done
})"
header_manifest_sha="$(printf '%s\n' "$header_manifest" | shasum -a 256 | awk '{print $1}')"
[[ "${#headers[@]}" == "$HOSTGRAPHICS_HEADER_COUNT" &&
   "$header_manifest_sha" == "$HOSTGRAPHICS_HEADER_MANIFEST_SHA256" ]] ||
  fail_build "header manifest mismatch expected=$HOSTGRAPHICS_HEADER_MANIFEST_SHA256 actual=$header_manifest_sha"

required_dirs=(
  "$source_root/include"
  "$nativedisplay_include"
  "$libhardware_include"
  "$aosp_root/frameworks/native/include"
  "$aosp_root/frameworks/native/libs/arect/include"
  "$aosp_root/frameworks/native/libs/math/include"
  "$aosp_root/frameworks/native/libs/nativebase/include"
  "$aosp_root/frameworks/native/libs/nativewindow/include"
  "$aosp_root/frameworks/native/libs/ui/include"
  "$aosp_root/frameworks/native/libs/ui/include_types"
  "$aosp_root/system/core/libcutils/include"
  "$aosp_root/system/core/libsystem/include"
  "$aosp_root/system/core/libutils/include"
  "$aosp_root/system/libbase/include"
  "$aosp_root/system/logging/liblog/include"
)
for dir in "${required_dirs[@]}"; do
  [[ -d "$dir" ]] || fail_build "missing exported include tree $dir"
done

cxx="$(command -v clang++ || true)"
[[ -n "$cxx" ]] || fail_build "clang++ is required"
ar="$(xcrun --find ar)"
sdk_root="$(xcrun --sdk macosx --show-sdk-path)"

mkdir -p "$output_dir"
stage="$(mktemp -d "$output_dir/stage.XXXXXX")"
trap 'rm -rf "$stage"' EXIT

flags=(
  -arch arm64 -isysroot "$sdk_root" -std=c++20 -O2 -fPIC
  -fvisibility=hidden -Wall -Werror -Wextra -Wno-unused-parameter
  -include type_traits
  -I"$source_root/include"
  -I"$nativedisplay_include"
  -I"$libhardware_include"
  -I"$aosp_root/frameworks/native/include"
  -I"$aosp_root/frameworks/native/libs/arect/include"
  -I"$aosp_root/frameworks/native/libs/math/include"
  -I"$aosp_root/frameworks/native/libs/nativebase/include"
  -I"$aosp_root/frameworks/native/libs/nativewindow/include"
  -I"$aosp_root/frameworks/native/libs/ui/include"
  -I"$aosp_root/frameworks/native/libs/ui/include_types"
  -I"$aosp_root/system/core/libcutils/include"
  -I"$aosp_root/system/core/libsystem/include"
  -I"$aosp_root/system/core/libutils/include"
  -I"$aosp_root/system/libbase/include"
  -I"$aosp_root/system/logging/liblog/include"
)

objects=()
for source in "${sources[@]}"; do
  object="$stage/${source%.cpp}.o"
  echo "hostgraphics-build: compile $source"
  "$cxx" "${flags[@]}" -c "$source_root/$source" -o "$object"
  objects+=("$object")
done

archive="$stage/libhostgraphics-darwin.a"
"$ar" rcs "$archive" "${objects[@]}"
file "$archive" | grep -F 'current ar archive' >/dev/null ||
  fail_build "output is not an ar archive"
architectures="$(lipo -archs "$archive")"
[[ "$architectures" == "arm64" ]] ||
  fail_build "archive architecture expected=arm64 actual=$architectures"
member_count="$("$ar" -t "$archive" | grep -v '^__\.SYMDEF' | wc -l | tr -d ' ')"
[[ "$member_count" == "$HOSTGRAPHICS_SOURCE_COUNT" ]] ||
  fail_build "archive member count expected=$HOSTGRAPHICS_SOURCE_COUNT actual=$member_count"

definitions="$(nm -gUC "$archive")"
for symbol in \
  '_ANativeWindow_dequeueBuffer' \
  '_ANativeWindow_tryAllocateBuffers' \
  'android::ADisplay_acquirePhysicalDisplays(android::ADisplay***)' \
  'android::ADisplay_getCurrentConfig(android::ADisplay*, android::ADisplayConfig**)'; do
  grep -F " T $symbol" <<<"$definitions" >/dev/null ||
    fail_build "missing representative definition $symbol"
done

printf '%s\n' "$source_manifest" > "$stage/source-manifest.txt"
nm -u "$archive" | awk '$1 ~ /^_/ { print $1 }' | sort -u > "$stage/archive-undefined-symbols.txt"
mv "$archive" "$output_dir/libhostgraphics-darwin.a"
mv "$stage/source-manifest.txt" "$output_dir/source-manifest.txt"
mv "$stage/archive-undefined-symbols.txt" "$output_dir/archive-undefined-symbols.txt"

echo "hostgraphics-build: objects=$member_count architecture=$architectures"
echo "hostgraphics-build: archive=$output_dir/libhostgraphics-darwin.a"
echo "hostgraphics-build: unresolved-manifest=$output_dir/archive-undefined-symbols.txt"
