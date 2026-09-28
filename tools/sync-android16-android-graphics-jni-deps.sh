#!/bin/bash
set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/.." && pwd)"
aosp="$project_root/_aosp"
lock_file="$project_root/upstream/android16-android-graphics-jni.lock"

[[ -f "$lock_file" ]] || { echo "android-graphics-jni-sync: missing $lock_file" >&2; exit 2; }
# shellcheck disable=SC1090
source "$lock_file"
source "$project_root/upstream/android16-ndk-bitmap.lock"
source "$project_root/upstream/android16-ndk-image-decoder.lock"

sha256() { shasum -a 256 "$1" | awk '{print $1}'; }
verify_file() {
  local path="$1" expected="$2"
  [[ -f "$path" ]] || { echo "android-graphics-jni-sync: missing $path" >&2; exit 3; }
  local actual
  actual="$(sha256 "$path")"
  [[ "$actual" == "$expected" ]] || {
    echo "android-graphics-jni-sync: identity mismatch: $path" >&2
    echo "expected=$expected actual=$actual" >&2
    exit 3
  }
}
fetch_file() {
  local project="$1" revision="$2" relative="$3" destination="$4" expected="$5"
  if [[ -e "$destination" ]]; then
    verify_file "$destination" "$expected"
    return
  fi
  mkdir -p "$(dirname "$destination")"
  local temporary
  temporary="$(mktemp "${destination}.tmp.XXXXXX")"
  curl -fsSL "https://android.googlesource.com/$project/+/$revision/$relative?format=TEXT" \
    | python3 -c 'import base64, sys; sys.stdout.buffer.write(base64.b64decode(sys.stdin.buffer.read()))' \
    > "$temporary"
  if [[ "$(sha256 "$temporary")" != "$expected" ]]; then
    echo "android-graphics-jni-sync: downloaded identity mismatch: $project/$relative" >&2
    find "$temporary" -delete
    exit 3
  fi
  mv "$temporary" "$destination"
}
fetch_archive() {
  local project="$1" revision="$2" subtree="$3" destination="$4" marker="$5" marker_hash="$6"
  if [[ -e "$destination" ]]; then
    verify_file "$destination/$marker" "$marker_hash"
    return
  fi
  mkdir -p "$(dirname "$destination")"
  local temporary
  temporary="$(mktemp -d "$(dirname "$destination")/.android-graphics-jni-sync.XXXXXX")"
  curl -fsSL "https://android.googlesource.com/$project/+archive/$revision/$subtree.tar.gz" \
    | tar -xzf - -C "$temporary"
  verify_file "$temporary/$marker" "$marker_hash"
  mv "$temporary" "$destination"
}

fetch_file "$NDK_BITMAP_PROJECT" "$NDK_BITMAP_REVISION" native/graphics/jni/bitmap.cpp \
  "$aosp/frameworks/base/native/graphics/jni/bitmap.cpp" "$NDK_BITMAP_SHA256"

for entry in \
  "imagedecoder.cpp:$NDK_IMAGE_DECODER_SHA256" \
  "aassetstreamadaptor.cpp:$NDK_ASSET_STREAM_SHA256" \
  "aassetstreamadaptor.h:$NDK_ASSET_STREAM_HEADER_SHA256" \
  "libjnigraphics.map.txt:$NDK_JNIGRAPHICS_MAP_SHA256" \
  "Android.bp:$NDK_JNIGRAPHICS_ANDROID_BP_SHA256"; do
  relative="native/graphics/jni/${entry%%:*}"
  fetch_file "$NDK_IMAGE_DECODER_PROJECT" "$NDK_IMAGE_DECODER_REVISION" "$relative" \
    "$aosp/frameworks/base/$relative" "${entry#*:}"
done

libjpeg="$aosp/external/libjpeg-turbo"
fetch_file "$LIBJPEG_TURBO_PROJECT" "$LIBJPEG_TURBO_REVISION" Android.bp \
  "$libjpeg/Android.bp" "$LIBJPEG_TURBO_ANDROID_BP_SHA256"
fetch_file "$LIBJPEG_TURBO_PROJECT" "$LIBJPEG_TURBO_REVISION" jpeglib.h \
  "$libjpeg/jpeglib.h" "$LIBJPEG_TURBO_PUBLIC_HEADER_SHA256"
fetch_file "$LIBJPEG_TURBO_PROJECT" "$LIBJPEG_TURBO_REVISION" jpeglibmangler.h \
  "$libjpeg/jpeglibmangler.h" "$LIBJPEG_TURBO_MANGLER_HEADER_SHA256"
fetch_file "$LIBJPEG_TURBO_PROJECT" "$LIBJPEG_TURBO_REVISION" jconfig.h \
  "$libjpeg/jconfig.h" "$LIBJPEG_TURBO_CONFIG_HEADER_SHA256"
fetch_file "$LIBJPEG_TURBO_PROJECT" "$LIBJPEG_TURBO_REVISION" jmorecfg.h \
  "$libjpeg/jmorecfg.h" "$LIBJPEG_TURBO_MORECFG_HEADER_SHA256"
fetch_file "$LIBJPEG_TURBO_PROJECT" "$LIBJPEG_TURBO_REVISION" jerror.h \
  "$libjpeg/jerror.h" "$LIBJPEG_TURBO_ERROR_HEADER_SHA256"

libultrahdr="$aosp/external/libultrahdr"
fetch_file "$LIBULTRAHDR_PROJECT" "$LIBULTRAHDR_REVISION" Android.bp \
  "$libultrahdr/Android.bp" "$LIBULTRAHDR_ANDROID_BP_SHA256"
fetch_file "$LIBULTRAHDR_PROJECT" "$LIBULTRAHDR_REVISION" ultrahdr_api.h \
  "$libultrahdr/ultrahdr_api.h" "$LIBULTRAHDR_API_HEADER_SHA256"
fetch_archive "$LIBULTRAHDR_PROJECT" "$LIBULTRAHDR_REVISION" lib/include \
  "$libultrahdr/lib/include" ultrahdr/jpegr.h "$LIBULTRAHDR_JPEGR_HEADER_SHA256"

fetch_archive "$FRAMEWORKS_NATIVE_PROJECT" "$FRAMEWORKS_NATIVE_REVISION" libs/gui/include \
  "$aosp/frameworks/native/libs/gui/include" gui/TraceUtils.h "$FRAMEWORKS_NATIVE_GUI_TRACE_HEADER_SHA256"
fetch_archive "$FRAMEWORKS_AV_PROJECT" "$FRAMEWORKS_AV_REVISION" media/ndk/include \
  "$aosp/frameworks/av/media/ndk/include" media/NdkImage.h "$FRAMEWORKS_AV_NDK_IMAGE_HEADER_SHA256"
fetch_archive "$LIBHARDWARE_PROJECT" "$LIBHARDWARE_REVISION" include_all \
  "$aosp/hardware/libhardware/include_all" hardware/hardware.h "$LIBHARDWARE_HARDWARE_HEADER_SHA256"

echo "android-graphics-jni-sync: history-free sparse dependencies ready"
