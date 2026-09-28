#!/bin/bash
set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/.." && pwd)"
lock_file="$project_root/upstream/android16-hwui.lock"

lock_value() {
  local key="$1"
  awk -F= -v key="$key" '$1 == key { print substr($0, index($0, "=") + 1); exit }' \
    "$lock_file"
}

verify_file() {
  local file="$1"
  local expected="$2"
  local actual
  actual="$(shasum -a 256 "$file" | awk '{print $1}')"
  if [[ "$actual" != "$expected" ]]; then
    echo "checksum mismatch: $file" >&2
    echo "expected $expected" >&2
    echo "actual   $actual" >&2
    return 1
  fi
}

verify_tree() {
  local source_root="$1"
  verify_file "$source_root/Android.bp" "$(lock_value ANDROID_BP_SHA256)"
  verify_file "$source_root/SkiaCanvas.cpp" "$(lock_value SKIA_CANVAS_SHA256)"
  verify_file "$source_root/hwui/Canvas.cpp" "$(lock_value CANVAS_CORE_SHA256)"
  verify_file "$source_root/jni/android_graphics_Canvas.cpp" \
    "$(lock_value CANVAS_JNI_SHA256)"
  verify_file "$source_root/jni/Paint.cpp" "$(lock_value PAINT_JNI_SHA256)"
}

revision="$(lock_value FRAMEWORKS_BASE_REVISION)"
subtree="$(lock_value FRAMEWORKS_BASE_SUBTREE)"
download_dir="$project_root/_downloads"
archive="$download_dir/frameworks-base-hwui-$revision.tar.gz"
destination="$project_root/_aosp/frameworks/base/libs/hwui"

if [[ -d "$destination" ]]; then
  verify_tree "$destination"
  echo "materialize-hwui: verified existing $destination"
  exit 0
fi

mkdir -p "$download_dir"
if [[ ! -f "$archive" ]]; then
  partial="$archive.partial"
  curl -fL --retry 3 \
    "https://android.googlesource.com/platform/frameworks/base/+archive/$revision/$subtree.tar.gz" \
    -o "$partial"
  mv "$partial" "$archive"
fi

staging_dir="$(mktemp -d "${TMPDIR:-/tmp}/aim-hwui.XXXXXX")"
cleanup() {
  rm -rf -- "$staging_dir"
}
trap cleanup EXIT
tar -xzf "$archive" -C "$staging_dir"
verify_tree "$staging_dir"

mkdir -p "$(dirname "$destination")"
mv "$staging_dir" "$destination"
trap - EXIT
echo "materialize-hwui: revision=$revision source=$destination"
