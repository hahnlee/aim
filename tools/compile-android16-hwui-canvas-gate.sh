#!/bin/bash
set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/.." && pwd)"
aosp_root="$project_root/_aosp"
output_dir="$project_root/_build/hwui-canvas-gate"
source_hwui="$aosp_root/frameworks/base/libs/hwui"
patched_hwui="$output_dir/patched-hwui"
critical_jni_patch="$project_root/patches/frameworks-base/0001-darwin-android-critical-jni-abi.patch"

"$script_dir/materialize-android16-hwui.sh"

required_paths=(
  frameworks/base/libs/hwui
  frameworks/base/libs/androidfw/include
  frameworks/native/include
  frameworks/native/libs/arect/include
  frameworks/native/libs/ui/include
  frameworks/native/libs/nativewindow/include
  frameworks/native/opengl/include
  frameworks/minikin/include
  system/core/libutils/include
  system/core/libcutils/include
  system/core/libsystem/include
  system/libbase/include
  system/incremental_delivery/incfs/util/include
  system/logging/liblog/include
  libnativehelper-full/include
  libnativehelper-full/include_platform
  libnativehelper/header_only_include
  libnativehelper/include_jni
  external/googletest/googletest/include
  external/harfbuzz_ng/src
  external/fmtlib/include
  external/vulkan-headers/include
  external/icu/icu4c/source/common
  external/skia/include/core
  external/skia/src/core
)

missing=()
for path in "${required_paths[@]}"; do
  if [[ ! -e "$aosp_root/$path" ]]; then
    missing+=("$path")
  fi
done
if (( ${#missing[@]} != 0 )); then
  echo "HWUI Canvas compile gate requires these revision-locked AOSP trees:" >&2
  printf '  _aosp/%s\n' "${missing[@]}" >&2
  echo "Their exact project revisions are recorded in upstream/android16-hwui.lock." >&2
  exit 2
fi

# Invoke the system driver rather than the toolchain binary returned by
# `xcrun --find`: the driver supplies the active macOS SDK's libc++ headers.
cxx="$(command -v clang++)"

# Keep the checksum-verified materialization immutable. The Darwin calling-ABI
# patch is deliberately applied only to a disposable build copy so it cannot
# hide source drift in materialize-android16-hwui.sh.
mkdir -p "$output_dir"
rm -rf -- "$patched_hwui"
cp -R "$source_hwui" "$patched_hwui"
patch -d "$patched_hwui" -p1 < "$critical_jni_patch"
hwui="$patched_hwui"

common_flags=(
  -std=c++23
  -arch arm64
  -fPIC
  -fno-rtti
  -fvisibility=hidden
  -DAIM_ANDROID_CRITICAL_JNI_ABI
  -DHWUI_NULL_GPU
  -DSK_BUILD_FOR_ANDROID_FRAMEWORK
  '-D__INTRODUCED_IN(n)='
  '-DSK_USER_CONFIG_HEADER="config/SkUserConfig.h"'
  -Wno-unused-parameter
  -Wno-deprecated-declarations
  -Wno-inconsistent-missing-override
  -Wno-abstract-final-class
  -Wno-deprecated-literal-operator
  -I"$project_root/compat/include"
  -iquote "$project_root/compat"
  -I"$hwui"
  -I"$hwui/jni"
  -I"$aosp_root/frameworks/base/libs/androidfw/include"
  -I"$aosp_root/system/core/libutils/include"
  -I"$aosp_root/system/core/libcutils/include"
  -I"$aosp_root/system/core/libsystem/include"
  -I"$aosp_root/system/incremental_delivery/incfs/util/include"
  -I"$aosp_root/frameworks/native/include"
  -I"$aosp_root/frameworks/native/libs/arect/include"
  -I"$aosp_root/frameworks/native/libs/ui/include"
  -I"$aosp_root/frameworks/native/libs/nativewindow/include"
  -I"$aosp_root/frameworks/native/opengl/include"
  -I"$aosp_root/frameworks/minikin/include"
  -I"$aosp_root/external/googletest/googletest/include"
  -I"$aosp_root/external/harfbuzz_ng/src"
  -I"$aosp_root/external/fmtlib/include"
  -I"$aosp_root/external/vulkan-headers/include"
  -I"$aosp_root/system/logging/liblog/include"
  -I"$aosp_root/system/libbase/include"
  -I"$aosp_root/libnativehelper-full/include"
  -I"$aosp_root/libnativehelper-full/include_platform"
  -I"$aosp_root/libnativehelper/header_only_include"
  -I"$aosp_root/libnativehelper/include_jni"
  -I"$aosp_root/external/icu/icu4c/source/common"
  -I"$aosp_root/external/skia"
  -I"$aosp_root/external/skia/client_utils/android"
  -I"$aosp_root/external/skia/include/core"
  -I"$aosp_root/external/skia/include/android"
  -I"$aosp_root/external/skia/include/utils"
  -I"$aosp_root/external/skia/include/effects"
  -I"$aosp_root/external/skia/include/codec"
  -I"$aosp_root/external/skia/include/gpu"
  -I"$aosp_root/external/skia/include/private"
  -I"$aosp_root/external/skia/src/core"
)

sources=(
  SkiaCanvas.cpp
  hwui/Canvas.cpp
  jni/android_graphics_Canvas.cpp
  jni/Paint.cpp
)

mkdir -p "$output_dir"
for relative_source in "${sources[@]}"; do
  object_name="${relative_source//\//_}.o"
  echo "compile-hwui-canvas: $relative_source"
  "$cxx" "${common_flags[@]}" -c "$hwui/$relative_source" \
    -o "$output_dir/$object_name"
done

file "$output_dir"/*.o
nm -gU "$output_dir/jni_android_graphics_Canvas.cpp.o" | \
  grep 'register_android_graphics_Canvas' >/dev/null
nm -gU "$output_dir/jni_Paint.cpp.o" | \
  grep 'register_android_graphics_Paint' >/dev/null

# These methods are representative @CriticalNative entries. They must use the
# Android ABI even though the surrounding HWUI implementation selects Darwin's
# host platform branches. A host ABI object would demangle with leading
# `_JNIEnv*, _jclass*` parameters and would be unsafe for ART to call.
canvas_symbols="$(nm -aC "$output_dir/jni_android_graphics_Canvas.cpp.o")"
paint_symbols="$(nm -aC "$output_dir/jni_Paint.cpp.o")"
grep -F 'android::CanvasJNI::getWidth(long long)' <<<"$canvas_symbols" >/dev/null
grep -F 'android::PaintGlue::setFlags(long long, int)' <<<"$paint_symbols" >/dev/null
if grep -F 'android::CanvasJNI::getWidth(_JNIEnv*, _jclass*' \
    <<<"$canvas_symbols" >/dev/null; then
  echo "compile-hwui-canvas: Canvas critical native uses host JNI ABI" >&2
  exit 1
fi
if grep -F 'android::PaintGlue::setFlags(_JNIEnv*, _jclass*' \
    <<<"$paint_symbols" >/dev/null; then
  echo "compile-hwui-canvas: Paint critical native uses host JNI ABI" >&2
  exit 1
fi
echo "compile-hwui-canvas: critical-jni-abi=android"

# Preserve the exact direct-object link closure for the next module-level gate.
# Platform C/C++ runtime symbols remain in this list by design.
nm -u "$output_dir"/*.o | awk '$1 ~ /^_/ { print $1 }' | sort -u \
  > "$output_dir/undefined-symbols.txt"
undefined_count="$(wc -l < "$output_dir/undefined-symbols.txt" | tr -d ' ')"
echo "compile-hwui-canvas: undefined-direct-object-symbols=$undefined_count"
echo "compile-hwui-canvas: objects=$output_dir"
