#!/bin/bash
set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/.." && pwd)"
lock_file="$project_root/upstream/android16-hostgraphics.lock"
source_root="$project_root/_aosp/frameworks/base/libs/hostgraphics"
deps_root="$project_root/_aosp/hostgraphics-deps"

# shellcheck disable=SC1090
source "$lock_file"

fail_sync() {
  echo "hostgraphics-sync: $1" >&2
  exit 2
}

download_file() {
  local project="$1" revision="$2" relative="$3" destination="$4"
  [[ -f "$destination" ]] && return
  mkdir -p "$(dirname "$destination")"
  local temporary
  temporary="$(mktemp "${destination}.partial.XXXXXX")"
  if ! curl -fsSL --retry 3 \
    "https://android.googlesource.com/$project/+/$revision/$relative?format=TEXT" | \
    base64 -D > "$temporary"; then
    rm -f "$temporary"
    fail_sync "download failed project=$project revision=$revision path=$relative"
  fi
  mv "$temporary" "$destination"
}

verify_sha() {
  local file="$1" expected="$2" actual
  [[ -f "$file" ]] || fail_sync "missing sparse input $file"
  actual="$(shasum -a 256 "$file" | awk '{print $1}')"
  [[ "$actual" == "$expected" ]] ||
    fail_sync "checksum mismatch file=$file expected=$expected actual=$actual"
}

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

download_file "$FRAMEWORKS_BASE_PROJECT" "$FRAMEWORKS_BASE_REVISION" \
  "$HOSTGRAPHICS_SUBTREE/Android.bp" "$source_root/Android.bp"
for file in "${sources[@]}" "${headers[@]}"; do
  download_file "$FRAMEWORKS_BASE_PROJECT" "$FRAMEWORKS_BASE_REVISION" \
    "$HOSTGRAPHICS_SUBTREE/$file" "$source_root/$file"
done

nativedisplay_include="$deps_root/frameworks-native/libs/nativedisplay/include"
libhardware_include="$deps_root/libhardware/include_all"
download_file "$FRAMEWORKS_NATIVE_PROJECT" "$FRAMEWORKS_NATIVE_REVISION" \
  libs/nativedisplay/include/apex/display.h "$nativedisplay_include/apex/display.h"
download_file "$LIBHARDWARE_PROJECT" "$LIBHARDWARE_REVISION" \
  include_all/hardware/hardware.h "$libhardware_include/hardware/hardware.h"

verify_sha "$source_root/Android.bp" "$HOSTGRAPHICS_ANDROID_BP_SHA256"
verify_sha "$nativedisplay_include/apex/display.h" "$NATIVEDISPLAY_DISPLAY_H_SHA256"
verify_sha "$libhardware_include/hardware/hardware.h" "$LIBHARDWARE_HARDWARE_H_SHA256"

source_manifest="$({
  for file in "${sources[@]}"; do
    [[ -f "$source_root/$file" ]] || fail_sync "missing module source $source_root/$file"
    printf '%s  %s\n' "$(shasum -a 256 "$source_root/$file" | awk '{print $1}')" "$file"
  done
})"
source_manifest_sha="$(printf '%s\n' "$source_manifest" | shasum -a 256 | awk '{print $1}')"
[[ "${#sources[@]}" == "$HOSTGRAPHICS_SOURCE_COUNT" &&
   "$source_manifest_sha" == "$HOSTGRAPHICS_SOURCE_MANIFEST_SHA256" ]] ||
  fail_sync "source manifest mismatch expected=$HOSTGRAPHICS_SOURCE_MANIFEST_SHA256 actual=$source_manifest_sha"

header_manifest="$({
  for file in "${headers[@]}"; do
    [[ -f "$source_root/$file" ]] || fail_sync "missing module header $source_root/$file"
    printf '%s  %s\n' "$(shasum -a 256 "$source_root/$file" | awk '{print $1}')" "$file"
  done
})"
header_manifest_sha="$(printf '%s\n' "$header_manifest" | shasum -a 256 | awk '{print $1}')"
[[ "${#headers[@]}" == "$HOSTGRAPHICS_HEADER_COUNT" &&
   "$header_manifest_sha" == "$HOSTGRAPHICS_HEADER_MANIFEST_SHA256" ]] ||
  fail_sync "header manifest mismatch expected=$HOSTGRAPHICS_HEADER_MANIFEST_SHA256 actual=$header_manifest_sha"

echo "hostgraphics-sync: revision=$FRAMEWORKS_BASE_REVISION"
echo "hostgraphics-sync: source=$source_root"
echo "hostgraphics-sync: deps=$deps_root"
