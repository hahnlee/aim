#!/bin/bash
set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/.." && pwd)"
aosp="$project_root/_aosp"
source_hwui="$aosp/frameworks/base/libs/hwui"
build_dir="$project_root/_build/android-graphics-jni"
patched_hwui="$build_dir/patched-hwui"
object_dir="$build_dir/objects"
lock_file="$project_root/upstream/android16-android-graphics-jni.lock"
gpu_lock_file="$project_root/upstream/android16-hwui-gpu.lock"
critical_patch="$project_root/patches/frameworks-base/0001-darwin-android-critical-jni-abi.patch"
lazy_native_window_patch="$project_root/patches/frameworks-base/0002-darwin-lazy-native-window-jni.patch"
require_jni_env_patch="$project_root/patches/frameworks-base/0011-darwin-hwui-require-jni-env.patch"
globalref_jni_patch="$project_root/patches/frameworks-base/0012-darwin-hwui-globalref-attach.patch"
thread_detach_patch="$project_root/patches/frameworks-base/0010-darwin-hwui-thread-detach.patch"
hwui_gpu_patch="$project_root/patches/frameworks-base/0003-darwin-hwui-gpu-layoutlib.patch"
mode=full
cpu_diagnostic=0

usage() {
  echo "usage: $0 [--registrar-only | --object-audit | --cpu-diagnostic]" >&2
}
case "${1:-}" in
  --registrar-only) mode=registrar; shift ;;
  --registrar-only-cpu) mode=registrar; cpu_diagnostic=1; shift ;;
  --object-audit) mode=objects; shift ;;
  --cpu-diagnostic) cpu_diagnostic=1; shift ;;
  --help|-h) usage; exit 0 ;;
  "") ;;
  *) usage; exit 64 ;;
esac
[[ $# -eq 0 ]] || { usage; exit 64; }

[[ -f "$lock_file" ]] || { echo "android-graphics-jni: missing $lock_file" >&2; exit 2; }
# shellcheck disable=SC1090
source "$lock_file"
source "$project_root/upstream/android16-ndk-bitmap.lock"
source "$project_root/upstream/android16-ndk-image-decoder.lock"
image_decoder_patch="$project_root/patches/frameworks-base/0017-ndk-image-decoder-input.patch"
image_decoder_rgba_patch="$project_root/patches/frameworks-base/0019-ndk-image-decoder-rgba.patch"
bitmap_buffer_patch="$project_root/patches/frameworks-base/0015-darwin-bitmap-buffer-access.patch"
bitmap_rgba_patch="$project_root/patches/frameworks-base/0016-ndk-bitmap-rgba-compression.patch"

sync_needed=0
[[ -f "$aosp/frameworks/base/native/graphics/jni/bitmap.cpp" ]] || sync_needed=1
[[ -f "$aosp/frameworks/base/native/graphics/jni/imagedecoder.cpp" ]] || sync_needed=1
[[ -n "${DARWIN_ART_LIBJPEG_TURBO_ROOT+x}" || -f "$aosp/external/libjpeg-turbo/Android.bp" ]] || sync_needed=1
[[ -n "${DARWIN_ART_LIBULTRAHDR_ROOT+x}" || -f "$aosp/external/libultrahdr/Android.bp" ]] || sync_needed=1
[[ -n "${DARWIN_ART_FRAMEWORKS_NATIVE_GUI_INCLUDE+x}" || -f "$aosp/frameworks/native/libs/gui/include/gui/TraceUtils.h" ]] || sync_needed=1
[[ -n "${DARWIN_ART_FRAMEWORKS_AV_MEDIA_NDK_INCLUDE+x}" || -f "$aosp/frameworks/av/media/ndk/include/media/NdkImage.h" ]] || sync_needed=1
[[ -n "${DARWIN_ART_LIBHARDWARE_INCLUDE+x}" || -f "$aosp/hardware/libhardware/include_all/hardware/hardware.h" ]] || sync_needed=1
if [[ "$mode" != registrar && "$sync_needed" -ne 0 ]]; then
  "$script_dir/sync-android16-android-graphics-jni-deps.sh"
fi

sha256() { shasum -a 256 "$1" | awk '{print $1}'; }
verify_hash() {
  local path="$1" expected="$2"
  if [[ ! -f "$path" ]]; then
    echo "android-graphics-jni: missing pinned source: $path" >&2
    exit 2
  fi
  local actual
  actual="$(sha256 "$path")"
  if [[ "$actual" != "$expected" ]]; then
    echo "android-graphics-jni: source identity mismatch: $path" >&2
    echo "expected=$expected actual=$actual" >&2
    exit 3
  fi
}

verify_hash "$source_hwui/Android.bp" "$HWUI_ANDROID_BP_SHA256"
verify_hash "$source_hwui/apex/LayoutlibLoader.cpp" "$LAYOUTLIB_LOADER_SHA256"
verify_hash "$source_hwui/apex/jni_runtime.cpp" "$JNI_RUNTIME_SHA256"
verify_hash "$source_hwui/jni/graphics_jni_helpers.h" "$GRAPHICS_JNI_HELPERS_UNPATCHED_SHA256"
verify_hash "$source_hwui/jni/PathIterator.cpp" "$PATH_ITERATOR_UNPATCHED_SHA256"
verify_hash "$source_hwui/jni/android_graphics_HardwareRenderer.cpp" "$HARDWARE_RENDERER_UNPATCHED_SHA256"
verify_hash "$source_hwui/platform/darwin/utils/SharedLib.cpp" "$DARWIN_SHARED_LIB_SHA256"
verify_hash "$critical_patch" "$CRITICAL_JNI_PATCH_SHA256"
verify_hash "$bitmap_buffer_patch" "$BITMAP_BUFFER_ACCESS_PATCH_SHA256"
verify_hash "$image_decoder_patch" "$NDK_IMAGE_DECODER_INPUT_PATCH_SHA256"
verify_hash "$image_decoder_rgba_patch" "$NDK_IMAGE_DECODER_RGBA_PATCH_SHA256"
verify_hash "$bitmap_rgba_patch" "$BITMAP_RGBA_COMPRESSION_PATCH_SHA256"
verify_hash "$lazy_native_window_patch" "$LAZY_NATIVE_WINDOW_PATCH_SHA256"
verify_hash "$globalref_jni_patch" "$GLOBALREF_JNI_PATCH_SHA256"
verify_hash "$thread_detach_patch" "$THREAD_DETACH_PATCH_SHA256"

mkdir -p "$build_dir" "$object_dir"
sources_file="$build_dir/android-graphics-jni-sources.txt"
source_manifest="$build_dir/android-graphics-jni-source-manifest.txt"
layoutlib_map_manifest="$build_dir/layoutlib-map-entries.txt"
registrar_manifest="$build_dir/runtime-ordered-layoutlib-registration.txt"
generated_dir="$build_dir/generated"
registration_csv="$generated_dir/android-graphics-native-classes.csv"
registration_header="$generated_dir/darwin_android_graphics_registration.h"
python3 - "$source_hwui/Android.bp" > "$sources_file" <<'PY'
import re
import sys
from pathlib import Path

blueprint = Path(sys.argv[1]).read_text()
module = blueprint.index('name: "android_graphics_jni"')
start = blueprint.index('srcs: [', module) + len('srcs: [')
end = blueprint.index('],', start)
print('\n'.join(re.findall(r'"([^"]+)"', blueprint[start:end])))
PY
source_count="$(wc -l < "$sources_file" | tr -d ' ')"
if [[ "$source_count" != "$ANDROID_GRAPHICS_JNI_SOURCE_COUNT" ]]; then
  echo "android-graphics-jni: Android.bp source count changed: expected=$ANDROID_GRAPHICS_JNI_SOURCE_COUNT actual=$source_count" >&2
  exit 3
fi
while IFS= read -r relative_source; do
  verify_hash "$source_hwui/$relative_source" "$(sha256 "$source_hwui/$relative_source")"
  printf '%s  %s\n' "$(sha256 "$source_hwui/$relative_source")" "$relative_source"
done < "$sources_file" > "$source_manifest"
source_manifest_sha="$(sha256 "$source_manifest")"
if [[ "$source_manifest_sha" != "$ANDROID_GRAPHICS_JNI_SOURCE_MANIFEST_SHA256" ]]; then
  echo "android-graphics-jni: ordered source content manifest changed" >&2
  echo "expected=$ANDROID_GRAPHICS_JNI_SOURCE_MANIFEST_SHA256 actual=$source_manifest_sha" >&2
  exit 3
fi

python3 - "$source_hwui/apex/LayoutlibLoader.cpp" > "$layoutlib_map_manifest" <<'PY'
import re
import sys
from pathlib import Path

source = Path(sys.argv[1]).read_text()
entries = re.findall(r'\{"([^"]+)",\s*(?:\n\s*)?REG_JNI\(([^)]+)\)\}', source)
for class_name, function in entries:
    print(f"{class_name} -> {function}")
PY
layoutlib_map_count="$(wc -l < "$layoutlib_map_manifest" | tr -d ' ')"
layoutlib_map_sha="$(sha256 "$layoutlib_map_manifest")"
if [[ "$layoutlib_map_count" != "$LAYOUTLIB_MAP_ENTRY_COUNT" ||
      "$layoutlib_map_sha" != "$LAYOUTLIB_MAP_ENTRY_MANIFEST_SHA256" ]]; then
  echo "android-graphics-jni: Layoutlib map entry set changed" >&2
  echo "expected_count=$LAYOUTLIB_MAP_ENTRY_COUNT actual_count=$layoutlib_map_count" >&2
  echo "expected_sha=$LAYOUTLIB_MAP_ENTRY_MANIFEST_SHA256 actual_sha=$layoutlib_map_sha" >&2
  exit 3
fi
python3 - "$source_hwui/apex/LayoutlibLoader.cpp" \
    "$source_hwui/apex/jni_runtime.cpp" > "$registrar_manifest" <<'PY'
import re
import sys
from pathlib import Path

layoutlib = Path(sys.argv[1]).read_text()
runtime = Path(sys.argv[2]).read_text()
entries = re.findall(r'\{"([^"]+)",\s*(?:\n\s*)?REG_JNI\(([^)]+)\)\}', layoutlib)
function_to_class = {function: class_name for class_name, function in entries}
if len(function_to_class) != len(entries):
    raise SystemExit("Layoutlib registration functions are not unique")
start = runtime.index("static const RegJNIRec gRegJNI[]")
end = runtime.index("};", start)
runtime_functions = re.findall(r'REG_JNI\(([^)]+)\)', runtime[start:end])
ordered = [(function_to_class[function], function)
           for function in runtime_functions if function in function_to_class]
if len(ordered) != len(entries) or {function for _, function in ordered} != set(function_to_class):
    raise SystemExit("jni_runtime gRegJNI does not cover the Layoutlib registrar exactly")
for class_name, function in ordered:
    print(f"{class_name} -> {function}")
PY
registration_count="$(wc -l < "$registrar_manifest" | tr -d ' ')"
registration_sha="$(sha256 "$registrar_manifest")"
if [[ "$registration_count" != "$FILTERED_RUNTIME_REGISTRATION_COUNT" ||
      "$registration_sha" != "$FILTERED_RUNTIME_ORDERED_REGISTRATION_SHA256" ]]; then
  echo "android-graphics-jni: filtered jni_runtime registration order changed" >&2
  echo "expected_count=$FILTERED_RUNTIME_REGISTRATION_COUNT actual_count=$registration_count" >&2
  echo "expected_sha=$FILTERED_RUNTIME_ORDERED_REGISTRATION_SHA256 actual_sha=$registration_sha" >&2
  exit 3
fi
mkdir -p "$generated_dir"
awk -F ' -> ' '{printf "%s%s", separator, $1; separator=","} END {printf "\n"}' \
  "$registrar_manifest" > "$registration_csv"
registration_csv_sha="$(sha256 "$registration_csv")"
if [[ "$registration_csv_sha" != "$FILTERED_RUNTIME_ORDERED_CLASS_CSV_SHA256" ]]; then
  echo "android-graphics-jni: generated ordered class CSV changed" >&2
  echo "expected=$FILTERED_RUNTIME_ORDERED_CLASS_CSV_SHA256 actual=$registration_csv_sha" >&2
  exit 3
fi
python3 - "$registration_csv" "$registration_header" "$registration_count" <<'PY'
import sys
from pathlib import Path

csv = Path(sys.argv[1]).read_text().strip()
output = Path(sys.argv[2])
count = int(sys.argv[3])
output.write_text(f'''#pragma once

// Generated by filtering the checksum-verified Android 16 jni_runtime.cpp
// registration order through LayoutlibLoader.cpp's supported map.
// Set this value as the java.lang.System "graphics_native_classes" property
// before register_android_graphics_classes() runs.
namespace darwin_art::android_graphics {{
inline constexpr char kNativeClassesPropertyName[] = "graphics_native_classes";
inline constexpr char kNativeClassesCsv[] = "{csv}";
inline constexpr unsigned kNativeClassCount = {count};
}}  // namespace darwin_art::android_graphics
''')
PY

if [[ -d "$patched_hwui" ]]; then
  case "$patched_hwui" in
    "$build_dir"/*) find "$patched_hwui" -depth -delete ;;
    *) echo "android-graphics-jni: refusing unsafe patched-tree cleanup: $patched_hwui" >&2; exit 3 ;;
  esac
fi
cp -R "$source_hwui" "$patched_hwui"
patch -s -d "$patched_hwui" -p1 < "$critical_patch"
patch -s -d "$patched_hwui" -p1 < "$lazy_native_window_patch"
patch -s -d "$patched_hwui" -p1 < "$thread_detach_patch"
patch -s -d "$patched_hwui" -p1 < "$require_jni_env_patch"
patch -s -d "$patched_hwui" -p1 < "$globalref_jni_patch"
patch -s -d "$patched_hwui" -p1 < "$bitmap_buffer_patch"
patch -s -d "$patched_hwui" -p1 < "$bitmap_rgba_patch"
gpu_mode=1
if [[ "$cpu_diagnostic" == 1 ]]; then
  gpu_mode=0
else
  [[ -f "$gpu_lock_file" ]] || {
    echo "android-graphics-jni: missing GPU lock: $gpu_lock_file" >&2
    exit 3
  }
  # shellcheck disable=SC1090
  source "$gpu_lock_file"
  verify_hash "$source_hwui/apex/LayoutlibLoader.cpp" "$LAYOUTLIB_LOADER_SHA256"
  verify_hash "$hwui_gpu_patch" "$GPU_LAYOUTLIB_PATCH_SHA256"
  [[ -f "$hwui_gpu_patch" ]] || {
    echo "android-graphics-jni: missing GPU layoutlib patch: $hwui_gpu_patch" >&2
    exit 3
  }
  patch -s -d "$patched_hwui" -p1 < "$hwui_gpu_patch"
  gpu_mode=1
fi
verify_hash "$patched_hwui/jni/graphics_jni_helpers.h" "$GRAPHICS_JNI_HELPERS_PATCHED_SHA256"
verify_hash "$patched_hwui/jni/android_graphics_HardwareRenderer.cpp" "$HARDWARE_RENDERER_PATCHED_SHA256"
python3 - "$patched_hwui/jni/PathIterator.cpp" <<'PY'
import sys
from pathlib import Path

path = Path(sys.argv[1])
source = path.read_text()
old = "#ifdef __ANDROID__\n        jint result = next(iteratorHandle, reinterpret_cast<jlong>(points));"
new = "#if defined(__ANDROID__) || defined(DARWIN_ART_ANDROID_CRITICAL_JNI_ABI)\n        jint result = next(iteratorHandle, reinterpret_cast<jlong>(points));"
if source.count(old) != 1:
    raise SystemExit("PathIterator critical-JNI host call-site source changed")
path.write_text(source.replace(old, new))
PY
verify_hash "$patched_hwui/jni/PathIterator.cpp" "$PATH_ITERATOR_PATCHED_SHA256"

cxx="$(command -v clang++)"
ar="$(xcrun --find ar)"
libjpeg_root="${DARWIN_ART_LIBJPEG_TURBO_ROOT:-$aosp/external/libjpeg-turbo}"
libultrahdr_root="${DARWIN_ART_LIBULTRAHDR_ROOT:-$aosp/external/libultrahdr}"
gui_include="${DARWIN_ART_FRAMEWORKS_NATIVE_GUI_INCLUDE:-$aosp/frameworks/native/libs/gui/include}"
media_ndk_include="${DARWIN_ART_FRAMEWORKS_AV_MEDIA_NDK_INCLUDE:-$aosp/frameworks/av/media/ndk/include}"
libhardware_include="${DARWIN_ART_LIBHARDWARE_INCLUDE:-$aosp/hardware/libhardware/include_all}"

common_flags=(
  -std=c++23 -arch arm64 -fPIC -fno-rtti -fvisibility=hidden
  -DDARWIN_ART_ANDROID_CRITICAL_JNI_ABI
  -DGL_GLEXT_PROTOTYPES -DEGL_EGLEXT_PROTOTYPES -DU_USING_ICU_NAMESPACE=0
  '-D__INTRODUCED_IN(n)='
  '-DSK_USER_CONFIG_HEADER="include/config/SkUserConfigManual.h"'
  -Wall -Werror -Wunused -Wunreachable-code
  -Wno-unused-parameter -Wno-non-virtual-dtor -Wno-parentheses
  -Wno-unused-variable -Wno-vla-cxx-extension
  -Wno-deprecated-declarations -Wno-inconsistent-missing-override
  -Wno-abstract-final-class -Wno-deprecated-literal-operator
  -Wno-unused-const-variable -Wno-unused-function
  -I"$patched_hwui" -I"$patched_hwui/jni" -I"$patched_hwui/apex/include"
  -I"$patched_hwui/platform/host"
  -I"$aosp/frameworks/base/libs/androidfw/include"
  -I"$aosp/frameworks/native/include" -I"$aosp/frameworks/native/include/private"
  -I"$aosp/frameworks/native/libs/arect/include"
  -I"$aosp/frameworks/native/libs/nativebase/include"
  -I"$aosp/frameworks/native/libs/ui/include" -I"$aosp/frameworks/native/libs/nativewindow/include"
  -I"$aosp/frameworks/native/opengl/include" -I"$aosp/frameworks/minikin/include"
  -I"$aosp/system/core/libutils/include" -I"$aosp/system/core/libcutils/include"
  -I"$aosp/system/core/libsystem/include" -I"$aosp/system/libbase/include"
  -I"$aosp/system/incremental_delivery/incfs/util/include" -I"$aosp/system/logging/liblog/include"
  -I"$aosp/libnativehelper-full/include" -I"$aosp/libnativehelper-full/include_platform"
  -I"$aosp/libnativehelper-full/include_platform_header_only"
  -I"$aosp/libnativehelper/header_only_include" -I"$aosp/libnativehelper/include_jni"
  -I"$aosp/external/fmtlib/include" -I"$aosp/external/harfbuzz_ng/src"
  -I"$aosp/external/googletest/googletest/include"
  -I"$aosp/external/icu/icu4c/source/common" -I"$aosp/external/icu/icu4c/source/i18n"
  -I"$aosp/external/freetype/include" -I"$aosp/external/libpng" -I"$aosp/external/zlib"
  -I"$aosp/external/vulkan-headers/include"
  -I"$aosp/external/skia" -I"$aosp/external/skia/client_utils/android"
  -I"$aosp/external/skia/include/core" -I"$aosp/external/skia/include/android"
  -I"$aosp/external/skia/include/utils" -I"$aosp/external/skia/include/effects"
  -I"$aosp/external/skia/include/pathops"
  -I"$aosp/external/skia/include/codec" -I"$aosp/external/skia/include/gpu"
  -I"$aosp/external/skia/include/private" -I"$aosp/external/skia/src/core"
  -I"$aosp/external/skia/src/codec"
)
common_flags+=( -iquote "$project_root/compat" )
if [[ "$gpu_mode" == 1 ]]; then
  common_flags+=( -DDARWIN_ART_HWUI_GPU )
else
  common_flags+=( -DHWUI_NULL_GPU )
fi

# Object files are shared across builds, but their sources come from a
# materialized tree assembled from several tracked patches. Pin the cache to
# that complete patch identity so header-only changes cannot reuse stale JNI
# objects.
patch_identity="$(for patch_file in "$critical_patch" "$lazy_native_window_patch" \
    "$thread_detach_patch" "$require_jni_env_patch" "$globalref_jni_patch" \
    "$hwui_gpu_patch" "$bitmap_buffer_patch" "$bitmap_rgba_patch" "$project_root/compat/darwin_hwui_jni_attachment.h"; do sha256 "$patch_file"; done |
    shasum -a 256 | awk '{print $1}')"

compile_cached() {
  local label="$1" source="$2" object="$3"
  shift 3
  local meta="${object}.cmd"
  local command_file="${object}.command"
  local source_sha
  source_sha="$(sha256 "$source")"
  local -a command=("$cxx" "${common_flags[@]}" "$@" -MMD -MF "$object.d" -c "$source" -o "$object")
  local command_text
  command_text="$(printf '%q ' "${command[@]}")"
  local key
  key="$(printf '%s\n%s\n%s\n' "$patch_identity" "$source_sha" "$command_text" |
    shasum -a 256 | awk '{print $1}')"
  if [[ -f "$object" && -f "$meta" && -f "$command_file" && "$(<"$meta")" == "$key" ]]; then
    echo "android-graphics-jni: cache $label"
    return
  fi
  echo "android-graphics-jni: compile $label"
  "${command[@]}"
  printf '%s\n' "$key" > "$meta"
  printf '%s\n' "$command_text" > "${command_file}.tmp.$$"
  mv "${command_file}.tmp.$$" "$command_file"
}

registrar_object="$object_dir/LayoutlibLoader.o"
compile_cached "apex/LayoutlibLoader.cpp" "$patched_hwui/apex/LayoutlibLoader.cpp" "$registrar_object"
if [[ "$(file "$registrar_object")" != *"Mach-O 64-bit object arm64"* ]]; then
  echo "android-graphics-jni: registrar is not an arm64 Mach-O object" >&2
  exit 3
fi
registrar_symbols="$(nm -gU "$registrar_object" | c++filt)"
for symbol in _init_android_graphics _register_android_graphics_classes _zygote_preload_graphics; do
  grep -F " T $symbol" <<<"$registrar_symbols" >/dev/null || {
    echo "android-graphics-jni: registrar definition missing: $symbol" >&2
    exit 3
  }
done
echo "android-graphics-jni: registrar-map=$registration_count sha256=$registration_sha arm64=1 gpu-mode=$gpu_mode"
if [[ "$mode" == registrar ]]; then
  exit 0
fi

missing_dependency=0
report_dependency_blocker() {
  local destination="$1" project="$2" revision="$3" required="$4"
  echo "android-graphics-jni: missing revision-locked dependency source" >&2
  echo "  destination=$destination" >&2
  echo "  project=$project revision=$revision" >&2
  echo "  required=$required" >&2
  echo "  archive=https://android.googlesource.com/$project/+archive/$revision.tar.gz" >&2
  missing_dependency=1
}
if [[ ! -f "$libjpeg_root/Android.bp" || ! -f "$libjpeg_root/jpeglib.h" ]]; then
  report_dependency_blocker "$libjpeg_root" "$LIBJPEG_TURBO_PROJECT" \
    "$LIBJPEG_TURBO_REVISION" "Android.bp,jpeglib.h"
else
  verify_hash "$libjpeg_root/Android.bp" "$LIBJPEG_TURBO_ANDROID_BP_SHA256"
  verify_hash "$libjpeg_root/jpeglib.h" "$LIBJPEG_TURBO_PUBLIC_HEADER_SHA256"
fi
if [[ ! -f "$libultrahdr_root/Android.bp" ||
      ! -f "$libultrahdr_root/lib/include/ultrahdr/jpegr.h" ||
      ! -f "$libultrahdr_root/ultrahdr_api.h" ]]; then
  report_dependency_blocker "$libultrahdr_root" "$LIBULTRAHDR_PROJECT" \
    "$LIBULTRAHDR_REVISION" "Android.bp,ultrahdr_api.h,lib/include/ultrahdr/jpegr.h"
else
  verify_hash "$libultrahdr_root/Android.bp" "$LIBULTRAHDR_ANDROID_BP_SHA256"
  verify_hash "$libultrahdr_root/lib/include/ultrahdr/jpegr.h" "$LIBULTRAHDR_JPEGR_HEADER_SHA256"
  verify_hash "$libultrahdr_root/ultrahdr_api.h" "$LIBULTRAHDR_API_HEADER_SHA256"
fi
if [[ ! -f "$gui_include/gui/TraceUtils.h" ]]; then
  echo "android-graphics-jni: missing revision-locked dependency source" >&2
  echo "  destination=$gui_include" >&2
  echo "  project=$FRAMEWORKS_NATIVE_PROJECT revision=$FRAMEWORKS_NATIVE_REVISION" >&2
  echo "  required=gui/TraceUtils.h" >&2
  echo "  archive=https://android.googlesource.com/$FRAMEWORKS_NATIVE_PROJECT/+archive/$FRAMEWORKS_NATIVE_REVISION/libs/gui/include.tar.gz" >&2
  missing_dependency=1
else
  verify_hash "$gui_include/gui/TraceUtils.h" "$FRAMEWORKS_NATIVE_GUI_TRACE_HEADER_SHA256"
fi
if [[ ! -f "$media_ndk_include/media/NdkImage.h" ]]; then
  echo "android-graphics-jni: missing revision-locked dependency source" >&2
  echo "  destination=$media_ndk_include" >&2
  echo "  project=$FRAMEWORKS_AV_PROJECT revision=$FRAMEWORKS_AV_REVISION" >&2
  echo "  required=media/NdkImage.h" >&2
  echo "  archive=https://android.googlesource.com/$FRAMEWORKS_AV_PROJECT/+archive/$FRAMEWORKS_AV_REVISION/media/ndk/include.tar.gz" >&2
  missing_dependency=1
else
  verify_hash "$media_ndk_include/media/NdkImage.h" "$FRAMEWORKS_AV_NDK_IMAGE_HEADER_SHA256"
fi
if [[ ! -f "$libhardware_include/hardware/hardware.h" ]]; then
  echo "android-graphics-jni: missing revision-locked dependency source" >&2
  echo "  destination=$libhardware_include" >&2
  echo "  project=$LIBHARDWARE_PROJECT revision=$LIBHARDWARE_REVISION" >&2
  echo "  required=hardware/hardware.h" >&2
  echo "  archive=https://android.googlesource.com/$LIBHARDWARE_PROJECT/+archive/$LIBHARDWARE_REVISION/include_all.tar.gz" >&2
  missing_dependency=1
else
  verify_hash "$libhardware_include/hardware/hardware.h" "$LIBHARDWARE_HARDWARE_HEADER_SHA256"
fi
if [[ "$missing_dependency" -ne 0 ]]; then
  exit 2
fi
common_flags+=( -I"$libjpeg_root" -I"$libultrahdr_root" -I"$libultrahdr_root/lib/include" -I"$gui_include" -I"$media_ndk_include" -I"$libhardware_include" )

objects=()
while IFS= read -r relative_source; do
  object_name="${relative_source//\//_}"
  object="$object_dir/${object_name%.cpp}.o"
  if [[ "$relative_source" == jni/android_graphics_HardwareRenderer.cpp ]]; then
    compile_cached "$relative_source" "$patched_hwui/$relative_source" "$object" \
      -DDARWIN_ART_LAZY_NATIVE_WINDOW_JNI
  else
    compile_cached "$relative_source" "$patched_hwui/$relative_source" "$object"
  fi
  objects+=("$object")
done < "$sources_file"
# Ordinary Android TextureView uses this Android-only JNI cohort. It is not
# part of the verified common source set or Layoutlib's 51-class registrar.
texture_layer_source="$patched_hwui/jni/android_graphics_TextureLayer.cpp"
verify_hash "$texture_layer_source" "$ANDROID_RUNTIME_TEXTURE_LAYER_SHA256"
texture_layer_object="$object_dir/jni_android_graphics_TextureLayer.o"
compile_cached "Android runtime: jni/android_graphics_TextureLayer.cpp" \
  "$texture_layer_source" "$texture_layer_object"
objects+=("$texture_layer_object")
darwin_shared_object="$object_dir/platform_darwin_utils_SharedLib.o"
compile_cached "platform/darwin/utils/SharedLib.cpp" \
  "$patched_hwui/platform/darwin/utils/SharedLib.cpp" "$darwin_shared_object"
objects+=("$darwin_shared_object")
# Original NDK entry points use AOSP's Bitmap owner for validation, pixel
# references, compression and hardware-buffer access. No handwritten mirror.
verify_hash "$aosp/frameworks/base/native/graphics/jni/bitmap.cpp" "$NDK_BITMAP_SHA256"
verify_hash "$source_hwui/apex/android_bitmap.cpp" "$APEX_BITMAP_SHA256"
verify_hash "$source_hwui/apex/include/android/graphics/bitmap.h" "$APEX_BITMAP_HEADER_SHA256"
compile_cached "native/graphics/jni/bitmap.cpp" \
  "$aosp/frameworks/base/native/graphics/jni/bitmap.cpp" "$object_dir/ndk_bitmap.o" \
  -fvisibility=default
compile_cached "apex/android_bitmap.cpp" \
  "$patched_hwui/apex/android_bitmap.cpp" "$object_dir/apex_android_bitmap.o"
objects+=("$object_dir/ndk_bitmap.o" "$object_dir/apex_android_bitmap.o")

# Keep the NDK decoder's public input contracts beside its original sources.
# The only Darwin implementation here transfers an owned host stream from the
# Rust Android descriptor table; decoding remains in AOSP ImageDecoder/Skia.
ndk_source="$aosp/frameworks/base/native/graphics/jni"
patched_ndk="$build_dir/patched-ndk"
mkdir -p "$patched_ndk"
verify_hash "$ndk_source/imagedecoder.cpp" "$NDK_IMAGE_DECODER_SHA256"
verify_hash "$ndk_source/aassetstreamadaptor.cpp" "$NDK_ASSET_STREAM_SHA256"
verify_hash "$ndk_source/aassetstreamadaptor.h" "$NDK_ASSET_STREAM_HEADER_SHA256"
cp "$ndk_source/imagedecoder.cpp" "$ndk_source/aassetstreamadaptor.cpp" \
  "$ndk_source/aassetstreamadaptor.h" "$patched_ndk/"
patch -s -d "$patched_ndk" -p1 < "$image_decoder_patch"
patch -s -d "$patched_ndk" -p1 < "$image_decoder_rgba_patch"
patch_identity="$(printf '%s\n%s\n%s\n' "$patch_identity" \
  "$(sha256 "$patched_ndk/aassetstreamadaptor.h")" \
  "$(sha256 "$project_root/compat/graphics/image_decoder_input.h")" |
  shasum -a 256 | awk '{print $1}')"
compile_cached "native/graphics/jni/imagedecoder.cpp" \
  "$patched_ndk/imagedecoder.cpp" "$object_dir/ndk_imagedecoder.o" -fvisibility=default
compile_cached "native/graphics/jni/aassetstreamadaptor.cpp" \
  "$patched_ndk/aassetstreamadaptor.cpp" "$object_dir/ndk_asset_stream.o"
compile_cached "graphics/image_decoder_input.cc" \
  "$project_root/compat/graphics/image_decoder_input.cc" "$object_dir/ndk_image_input.o" \
  -I"$project_root/tools/bionic-fs-facade/include" \
  -I"$project_root/tools/bionic-ioctl-facade/include"
objects+=("$object_dir/ndk_imagedecoder.o" "$object_dir/ndk_asset_stream.o" \
  "$object_dir/ndk_image_input.o")

jni_archive="$build_dir/libandroid-graphics-jni-darwin.a"
registrar_archive="$build_dir/libandroid-graphics-layoutlib-registrar-darwin.a"
rm -f "$jni_archive" "$registrar_archive"
"$ar" rcs "$jni_archive" "${objects[@]}"
"$ar" rcs "$registrar_archive" "$registrar_object"
jni_members="$({ "$ar" -t "$jni_archive" || true; } | grep -v '^__\.SYMDEF' | wc -l | tr -d ' ')"
[[ "$jni_members" == 67 ]] || { echo "android-graphics-jni: JNI archive member count=$jni_members expected=67" >&2; exit 3; }

combined_object="$build_dir/android-graphics-jni-force-loaded.o"
"$cxx" -r -arch arm64 -Wl,-force_load,"$registrar_archive" \
  -Wl,-force_load,"$jni_archive" -o "$combined_object"
combined_definitions="$(nm -aC "$combined_object")"
grep -F 'android::register_android_graphics_TextureLayer(_JNIEnv*)' \
  <<<"$combined_definitions" >/dev/null || {
  echo "android-graphics-jni: Android runtime TextureLayer registrar missing" >&2
  exit 3
}
while IFS= read -r registration; do
  class_name="${registration%% -> *}"
  function="${registration#* -> }"
  grep -E " [Tt] .*${function}\\(" <<<"$combined_definitions" >/dev/null || {
    echo "android-graphics-jni: registrar target not defined by force-loaded common host closure: $class_name -> $function" >&2
    exit 3
  }
done < "$registrar_manifest"

canvas_symbols="$(nm -aC "$object_dir/jni_android_graphics_Canvas.o")"
paint_symbols="$(nm -aC "$object_dir/jni_Paint.o")"
grep -F 'android::CanvasJNI::getWidth(long long)' <<<"$canvas_symbols" >/dev/null
grep -F 'android::PaintGlue::setFlags(long long, int)' <<<"$paint_symbols" >/dev/null
if grep -F 'android::CanvasJNI::getWidth(_JNIEnv*, _jclass*' <<<"$canvas_symbols" >/dev/null ||
   grep -F 'android::PaintGlue::setFlags(_JNIEnv*, _jclass*' <<<"$paint_symbols" >/dev/null; then
  echo "android-graphics-jni: critical native compiled with host JNI ABI" >&2
  exit 3
fi

nm -u "$combined_object" | awk '$1 ~ /^_/ { print $1 }' | sort -u \
  > "$build_dir/force-loaded-undefined-symbols.txt"
undefined_count="$(wc -l < "$build_dir/force-loaded-undefined-symbols.txt" | tr -d ' ')"
echo "android-graphics-jni: common-sources=$source_count android-runtime-extra=1 host-extra=1 archive-members=$jni_members critical-jni-abi=android"
echo "android-graphics-jni: force-load registrar=$registration_count unresolved-transitive=$undefined_count"
if [[ "$mode" == objects ]]; then
  exit 0
fi

fake_source="$project_root/compat/darwin_framework_natives.cc"
if [[ -f "$fake_source" ]] &&
   grep -F '"android/graphics/Paint"' "$fake_source" >/dev/null &&
   grep -F '"android/graphics/RenderNode"' "$fake_source" >/dev/null; then
  echo "android-graphics-jni: final integration blocked by overlapping native ownership" >&2
  echo "  current fake registrar=$fake_source" >&2
  echo "  overlaps=android/graphics/Paint,android/graphics/RenderNode" >&2
  echo "  upstream registrar=$source_hwui/apex/LayoutlibLoader.cpp" >&2
  echo "Fake and upstream native tables cannot coexist: they use incompatible native handles and duplicate class registration." >&2
  echo "Remove the fake Paint/RenderNode registration from the runtime composition only when the force-loaded GraphicsJNI/HWUI dependency closure is link-complete." >&2
  exit 2
fi

echo "android-graphics-jni: fake-overlap cleared; final executable link still requires module-complete libhwui, libandroidfw, libimage_io, libjpeg, libultrahdr, and current foundation archives" >&2
exit 2
