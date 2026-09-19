#!/bin/bash
set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/.." && pwd)"
aosp_root="$project_root/_aosp"
hwui="$aosp_root/frameworks/base/libs/hwui"
lock_file="$project_root/upstream/android16-hwui-static-foundation.lock"
output_dir="$project_root/_build/hwui-static-foundation"
animation_patch="$project_root/patches/frameworks-base/0005-darwin-hwui-animation-pulse.patch"
darwin_gpu_patch="$project_root/patches/frameworks-base/0006-darwin-hwui-gpu.patch"
darwin_renderthread_patch="$project_root/patches/frameworks-base/0007-darwin-hwui-renderthread.patch"
darwin_angle_surface_patch="$project_root/patches/frameworks-base/0008-darwin-hwui-angle-rgba-surface.patch"
darwin_wide_gamut_patch="$project_root/patches/frameworks-base/0009-darwin-hwui-unsupported-wide-gamut.patch"
darwin_thread_detach_patch="$project_root/patches/frameworks-base/0010-darwin-hwui-thread-detach.patch"
darwin_require_jni_patch="$project_root/patches/frameworks-base/0011-darwin-hwui-require-jni-env.patch"
darwin_common_pool_patch="$project_root/patches/frameworks-base/0013-darwin-hwui-common-pool-shutdown.patch"
darwin_common_pool_explicit_patch="$project_root/patches/frameworks-base/0014-darwin-hwui-common-pool-explicit-shutdown.patch"
darwin_cross_tu_abi_patch="$project_root/patches/frameworks-base/0019-darwin-hwui-export-cross-tu-abi.patch"

# shellcheck disable=SC1090
source "$lock_file"
source "$project_root/upstream/android16-ndk-bitmap.lock"
bitmap_buffer_patch="$project_root/patches/frameworks-base/0015-darwin-bitmap-buffer-access.patch"
source "$project_root/upstream/android16-ndk-image-decoder.lock"
decoder_color_patch="$project_root/patches/frameworks-base/0018-image-decoder-explicit-color-order.patch"

deps_root="$aosp_root/hwui-static-deps"
if [[ -z "${DARWIN_ART_ANDROID16_SYSPROP_ROOT:-}" ||
      -z "${DARWIN_ART_ANDROID16_SYSPROP_CPP:-}" ||
      -z "${DARWIN_ART_ANDROID16_GUI_INCLUDE:-}" ||
      -z "${DARWIN_ART_ANDROID16_BINDER_INCLUDE:-}" ||
      -z "${DARWIN_ART_ANDROID16_NATIVEDISPLAY_INCLUDE:-}" ||
      -z "${DARWIN_ART_ANDROID16_LIBHARDWARE_INCLUDE:-}" ]]; then
  "$script_dir/sync-android16-hwui-static-deps.sh"
fi

fail_source() {
  echo "hwui-static-foundation: $1" >&2
  exit 2
}

verify_sha() {
  local file="$1" expected="$2"
  [[ -f "$file" ]] || fail_source "missing source $file"
  local actual
  actual="$(shasum -a 256 "$file" | awk '{print $1}')"
  [[ "$actual" == "$expected" ]] ||
    fail_source "checksum mismatch file=$file expected=$expected actual=$actual"
}

verify_sha "$hwui/Android.bp" "$HWUI_ANDROID_BP_SHA256"
verify_sha "$bitmap_buffer_patch" "$BITMAP_BUFFER_ACCESS_PATCH_SHA256"
verify_sha "$decoder_color_patch" "$IMAGE_DECODER_COLOR_ORDER_PATCH_SHA256"
verify_sha "$hwui/HWUIProperties.sysprop" "$HWUI_SYSPROP_SHA256"
verify_sha "$project_root/patches/frameworks-base/0001-darwin-android-critical-jni-abi.patch" \
  "$CRITICAL_JNI_PATCH_SHA256"
verify_sha "$animation_patch" "$ANIMATION_PULSE_PATCH_SHA256"
verify_sha "$darwin_gpu_patch" "$DARWIN_GPU_PATCH_SHA256"
verify_sha "$darwin_renderthread_patch" "$DARWIN_RENDERTHREAD_PATCH_SHA256"
verify_sha "$darwin_angle_surface_patch" "$DARWIN_ANGLE_SURFACE_PATCH_SHA256"
verify_sha "$darwin_wide_gamut_patch" "$DARWIN_WIDE_GAMUT_PATCH_SHA256"
verify_sha "$darwin_common_pool_patch" "$DARWIN_COMMON_POOL_PATCH_SHA256"
verify_sha "$darwin_common_pool_explicit_patch" "$DARWIN_COMMON_POOL_EXPLICIT_PATCH_SHA256"
verify_sha "$darwin_cross_tu_abi_patch" "$HWUI_CROSS_TU_ABI_PATCH_SHA256"

sources=(
  canvas/CanvasFrontend.cpp
  canvas/CanvasOpBuffer.cpp
  canvas/CanvasOpRasterizer.cpp
  effects/StretchEffect.cpp
  effects/GainmapRenderer.cpp
  pipeline/skia/BackdropFilterDrawable.cpp
  pipeline/skia/HolePunch.cpp
  pipeline/skia/LayerDrawable.cpp
  pipeline/skia/SkiaCpuPipeline.cpp
  pipeline/skia/SkiaDisplayList.cpp
  pipeline/skia/SkiaGpuPipeline.cpp
  pipeline/skia/SkiaOpenGLPipeline.cpp
  pipeline/skia/SkiaPipeline.cpp
  pipeline/skia/SkiaProfileRenderer.cpp
  pipeline/skia/SkiaRecordingCanvas.cpp
  pipeline/skia/StretchMask.cpp
  pipeline/skia/RenderNodeDrawable.cpp
  pipeline/skia/ReorderBarrierDrawables.cpp
  pipeline/skia/TransformCanvas.cpp
  renderstate/RenderState.cpp
  renderthread/CanvasContext.cpp
  renderthread/DrawFrameTask.cpp
  renderthread/EglManager.cpp
  renderthread/Frame.cpp
  renderthread/RenderEffectCapabilityQuery.cpp
  renderthread/RenderProxy.cpp
  renderthread/RenderThread.cpp
  renderthread/RenderTask.cpp
  renderthread/TimeLord.cpp
  hwui/AnimatedImageDrawable.cpp
  hwui/AnimatedImageThread.cpp
  hwui/Bitmap.cpp
  hwui/BlurDrawLooper.cpp
  hwui/Canvas.cpp
  hwui/ImageDecoder.cpp
  hwui/MinikinSkia.cpp
  hwui/MinikinUtils.cpp
  hwui/PaintImpl.cpp
  hwui/Typeface.cpp
  thread/CommonPool.cpp
  utils/Blur.cpp
  utils/Color.cpp
  utils/GLUtils.cpp
  utils/LinearAllocator.cpp
  utils/StringUtils.cpp
  utils/StatsUtils.cpp
  utils/TypefaceUtils.cpp
  utils/VectorDrawableUtils.cpp
  AnimationContext.cpp
  Animator.cpp
  AnimatorManager.cpp
  AutoBackendTextureRelease.cpp
  CanvasTransform.cpp
  DamageAccumulator.cpp
  DeviceInfo.cpp
  DeferredLayerUpdater.cpp
  FrameInfo.cpp
  FrameInfoVisualizer.cpp
  FrameMetricsReporter.cpp
  Gainmap.cpp
  Interpolator.cpp
  JankTracker.cpp
  Layer.cpp
  LayerUpdateQueue.cpp
  LightingInfo.cpp
  Matrix.cpp
  Mesh.cpp
  MemoryPolicy.cpp
  PathParser.cpp
  ProfileData.cpp
  Properties.cpp
  PropertyValuesAnimatorSet.cpp
  PropertyValuesHolder.cpp
  RecordingCanvas.cpp
  RenderNode.cpp
  RenderProperties.cpp
  RootRenderNode.cpp
  SkiaCanvas.cpp
  SkiaInterpolator.cpp
  Tonemapper.cpp
  TreeInfo.cpp
  VectorDrawable.cpp
  platform/host/renderthread/CacheManager.cpp
  platform/host/renderthread/HintSessionWrapper.cpp
  platform/host/renderthread/ReliableSurface.cpp
  platform/host/ProfileDataContainer.cpp
  platform/host/Readback.cpp
  platform/host/WebViewFunctorManager.cpp
)

apex_common_sources=(
  apex/android_canvas.cpp
  apex/android_matrix.cpp
  apex/android_paint.cpp
  apex/android_region.cpp
  apex/properties.cpp
)

[[ "${#sources[@]}" == "$HWUI_SOURCE_COUNT" ]] ||
  fail_source "internal source count mismatch"
source_manifest="$({
  for source in "${sources[@]}"; do
    [[ -f "$hwui/$source" ]] || fail_source "missing module source $hwui/$source"
    printf '%s  %s\n' "$(shasum -a 256 "$hwui/$source" | awk '{print $1}')" "$source"
  done
})"
actual_manifest_sha="$(printf '%s\n' "$source_manifest" | shasum -a 256 | awk '{print $1}')"
[[ "$actual_manifest_sha" == "$HWUI_SOURCE_MANIFEST_SHA256" ]] ||
  fail_source "source manifest mismatch expected=$HWUI_SOURCE_MANIFEST_SHA256 actual=$actual_manifest_sha"
apex_manifest="$({
  for source in "${apex_common_sources[@]}"; do
    [[ -f "$hwui/$source" ]] || fail_source "missing APEX-common source $hwui/$source"
    printf '%s  %s\n' "$(shasum -a 256 "$hwui/$source" | awk '{print $1}')" "$source"
  done
})"
actual_apex_manifest_sha="$(printf '%s\n' "$apex_manifest" | shasum -a 256 | awk '{print $1}')"
[[ "${#apex_common_sources[@]}" == "$APEX_COMMON_SOURCE_COUNT" &&
   "$actual_apex_manifest_sha" == "$APEX_COMMON_SOURCE_MANIFEST_SHA256" ]] ||
  fail_source "APEX-common source manifest mismatch"

sysprop_root="${DARWIN_ART_ANDROID16_SYSPROP_ROOT:-$deps_root/system-tools-sysprop}"
sysprop_cpp="${DARWIN_ART_ANDROID16_SYSPROP_CPP:-$project_root/_build/hwui-static-deps/sysprop_cpp}"
[[ -n "$sysprop_root" && -n "$sysprop_cpp" && -x "$sysprop_cpp" ]] || {
  echo "hwui-static-foundation: pinned upstream sysprop_cpp is required" >&2
  echo "  project=$SYSPROP_PROJECT revision=$SYSPROP_REVISION" >&2
  echo "  set DARWIN_ART_ANDROID16_SYSPROP_ROOT and DARWIN_ART_ANDROID16_SYSPROP_CPP" >&2
  exit 2
}
verify_sha "$sysprop_root/Android.bp" "$SYSPROP_ANDROID_BP_SHA256"
verify_sha "$sysprop_root/sysprop.proto" "$SYSPROP_PROTO_SHA256"
verify_sha "$sysprop_root/Common.cpp" "$SYSPROP_COMMON_CPP_SHA256"
verify_sha "$sysprop_root/CodeWriter.cpp" "$SYSPROP_CODE_WRITER_CPP_SHA256"
verify_sha "$sysprop_root/CppGen.cpp" "$SYSPROP_CPP_GEN_CPP_SHA256"
verify_sha "$sysprop_root/CppMain.cpp" "$SYSPROP_CPP_MAIN_CPP_SHA256"

gui_include="${DARWIN_ART_ANDROID16_GUI_INCLUDE:-$deps_root/frameworks-native/libs/gui/include}"
binder_include="${DARWIN_ART_ANDROID16_BINDER_INCLUDE:-$deps_root/frameworks-native/libs/binder/include}"
nativedisplay_include="${DARWIN_ART_ANDROID16_NATIVEDISPLAY_INCLUDE:-$deps_root/frameworks-native/libs/nativedisplay/include}"
libhardware_include="${DARWIN_ART_ANDROID16_LIBHARDWARE_INCLUDE:-$deps_root/libhardware/include_all}"
verify_sha "$libhardware_include/hardware/hardware.h" "$LIBHARDWARE_HARDWARE_H_SHA256"

required_dirs=(
  "$gui_include"
  "$binder_include"
  "$nativedisplay_include"
  "$libhardware_include"
  "$aosp_root/frameworks/base/libs/androidfw/include"
  "$aosp_root/frameworks/native/include"
  "$aosp_root/frameworks/native/include/private"
  "$aosp_root/frameworks/native/libs/arect/include"
  "$aosp_root/frameworks/native/libs/math/include"
  "$aosp_root/frameworks/native/libs/nativebase/include"
  "$aosp_root/frameworks/native/libs/nativewindow/include"
  "$aosp_root/frameworks/native/libs/ui/include"
  "$aosp_root/frameworks/native/libs/ui/include_types"
  "$aosp_root/frameworks/native/opengl/include"
  "$aosp_root/frameworks/minikin/include"
  "$aosp_root/system/core/libcutils/include"
  "$aosp_root/system/core/libsystem/include"
  "$aosp_root/system/core/libutils/include"
  "$aosp_root/system/incremental_delivery/incfs/util/include"
  "$aosp_root/system/libbase/include"
  "$aosp_root/system/logging/liblog/include"
  "$aosp_root/external/fmtlib/include"
  "$aosp_root/external/googletest/googletest/include"
  "$aosp_root/external/harfbuzz_ng/src"
  "$aosp_root/external/icu-graphics/android_icu4c/include"
  "$aosp_root/external/icu-graphics/icu4c/source/common"
  "$aosp_root/external/skia"
  "$aosp_root/external/vulkan-headers/include"
  "$aosp_root/libnativehelper-full/include"
  "$aosp_root/libnativehelper-full/include_platform"
  "$aosp_root/libnativehelper/header_only_include"
  "$aosp_root/libnativehelper/include_jni"
)
for dir in "${required_dirs[@]}"; do
  [[ -d "$dir" ]] || fail_source "missing exported include tree $dir"
done

cxx="$(command -v clang++ || true)"
[[ -n "$cxx" ]] || fail_source "clang++ is required"
ar="$(xcrun --find ar)"
sdk_root="$(xcrun --sdk macosx --show-sdk-path)"

mkdir -p "$output_dir"
stage="$(mktemp -d "$output_dir/stage.XXXXXX")"
trap 'rm -rf "$stage"' EXIT
cache_dir="$output_dir/objects"
generated_dir="$output_dir/generated"
mkdir -p "$cache_dir" "$generated_dir/include" "$generated_dir/source" "$generated_dir/public"

# Keep Darwin-only HWUI edits in a tracked patch instead of mutating the
# ignored upstream checkout.  The shadow tree has a stable path so command
# stamps remain valid across invocations; it is rebuilt only when the patch
# or pinned source identity changes.
patched_hwui="$output_dir/patched-source"
patched_marker="$patched_hwui/.darwin-art-patched-source"
patch_identity="$(printf '%s\n%s\n%s\n%s\n%s\n%s\n' "$HWUI_SOURCE_MANIFEST_SHA256" \
  "$ANIMATION_PULSE_PATCH_SHA256" "$DARWIN_GPU_PATCH_SHA256" \
  "$DARWIN_RENDERTHREAD_PATCH_SHA256" "$DARWIN_ANGLE_SURFACE_PATCH_SHA256" \
  "$DARWIN_WIDE_GAMUT_PATCH_SHA256" "$DARWIN_THREAD_DETACH_PATCH_SHA256" \
  "$DARWIN_REQUIRE_JNI_PATCH_SHA256" \
  "$DARWIN_COMMON_POOL_PATCH_SHA256" \
  "$DARWIN_COMMON_POOL_EXPLICIT_PATCH_SHA256" \
  "$HWUI_CROSS_TU_ABI_PATCH_SHA256" \
  "$BITMAP_BUFFER_ACCESS_PATCH_SHA256" \
  "$IMAGE_DECODER_COLOR_ORDER_PATCH_SHA256" \
  "$(shasum -a 256 "$project_root/compat/darwin_hwui_jni_attachment.h" | awk '{print $1}')" \
  | shasum -a 256 | awk '{print $1}')"
if [[ ! -f "$patched_marker" || "$(<"$patched_marker")" != "$patch_identity" ]]; then
  fresh_shadow="$output_dir/patched-source.new.$$"
  mkdir -p "$fresh_shadow"
  cp -R "$hwui/." "$fresh_shadow/"
  patch -d "$fresh_shadow" -p1 < "$animation_patch"
  patch -d "$fresh_shadow" -p1 < "$darwin_gpu_patch"
  patch -d "$fresh_shadow" -p1 < "$darwin_renderthread_patch"
  patch -d "$fresh_shadow" -p1 < "$darwin_angle_surface_patch"
  patch -d "$fresh_shadow" -p1 < "$darwin_wide_gamut_patch"
  patch -d "$fresh_shadow" -p1 < "$darwin_thread_detach_patch"
  patch -d "$fresh_shadow" -p1 < "$darwin_require_jni_patch"
  patch -d "$fresh_shadow" -p1 < "$darwin_common_pool_patch"
  patch -d "$fresh_shadow" -p1 < "$darwin_common_pool_explicit_patch"
  # This patch is rooted at frameworks/base; the producer shadow is libs/hwui.
  patch --batch --forward -d "$fresh_shadow" -p3 < "$darwin_cross_tu_abi_patch"
  patch -d "$fresh_shadow" -p1 < "$bitmap_buffer_patch"
  patch -d "$fresh_shadow" -p1 < "$decoder_color_patch"
  printf '%s\n' "$patch_identity" > "$fresh_shadow/.darwin-art-patched-source"
  rm -rf "$patched_hwui"
  mv "$fresh_shadow" "$patched_hwui"
fi
hwui_source="$patched_hwui"
"$sysprop_cpp" \
  --header-dir "$generated_dir/include" \
  --source-dir "$generated_dir/source" \
  --include-name HWUIProperties.sysprop.h \
  --public-header-dir "$generated_dir/public" \
  "$hwui/HWUIProperties.sysprop"
verify_sha "$generated_dir/source/HWUIProperties.sysprop.cpp" \
  "$GENERATED_HWUI_PROPERTIES_CPP_SHA256"
verify_sha "$generated_dir/include/HWUIProperties.sysprop.h" \
  "$GENERATED_HWUI_PROPERTIES_H_SHA256"

flags=(
  -arch arm64 -isysroot "$sdk_root" -std=c++23 -O2 -fPIC -fno-rtti
  -fvisibility=hidden -Wall -Werror -Wextra -Wthread-safety
  '-D__INTRODUCED_IN(n)='
  '-DSK_USER_CONFIG_HEADER="include/config/SkUserConfigManual.h"'
  '-DLOG_TAG="OpenGLRenderer"' -DGL_GLEXT_PROTOTYPES -DEGL_EGLEXT_PROTOTYPES
  -DATRACE_TAG=ATRACE_TAG_VIEW
  -Wno-unknown-warning-option -Wno-invalid-specialization
  -Wno-unused-parameter -Wno-unused-private-field -Wno-deprecated-declarations
  -Wno-unused-const-variable
  -Wno-unused-variable
  -Wno-sign-compare
  -Wno-inconsistent-missing-override -Wno-abstract-final-class
  -Wno-deprecated-literal-operator -Wno-missing-field-initializers
  -I"$generated_dir/include"
  -iquote "$project_root/compat"
  -I"$project_root/compat/hwui-android-platform"
  -I"$hwui_source" -I"$hwui_source/platform/host"
  -I"$aosp_root/frameworks/base/libs/androidfw/include"
  -I"$aosp_root/frameworks/native/include"
  -I"$aosp_root/frameworks/native/include/private"
  -I"$aosp_root/frameworks/native/libs/arect/include"
  -I"$aosp_root/frameworks/native/libs/math/include"
  -I"$aosp_root/frameworks/native/libs/nativebase/include"
  -I"$aosp_root/frameworks/native/libs/nativewindow/include"
  -I"$aosp_root/frameworks/native/libs/ui/include"
  -I"$aosp_root/frameworks/native/libs/ui/include_types"
  -I"$aosp_root/frameworks/native/opengl/include"
  -I"$gui_include" -I"$binder_include" -I"$nativedisplay_include" -I"$libhardware_include"
  -I"$aosp_root/frameworks/minikin/include"
  -I"$aosp_root/system/core/libcutils/include"
  -I"$aosp_root/system/core/libsystem/include"
  -I"$aosp_root/system/core/libutils/include"
  -I"$aosp_root/system/incremental_delivery/incfs/util/include"
  -I"$aosp_root/system/libbase/include"
  -I"$aosp_root/system/logging/liblog/include"
  -I"$aosp_root/external/fmtlib/include"
  -I"$aosp_root/external/googletest/googletest/include"
  -I"$aosp_root/external/harfbuzz_ng/src"
  -I"$aosp_root/external/icu-graphics/android_icu4c/include"
  -I"$aosp_root/external/icu-graphics/icu4c/source/common"
  -I"$aosp_root/external/skia"
  -I"$aosp_root/external/skia/client_utils/android"
  -I"$aosp_root/external/skia/include/android"
  -I"$aosp_root/external/skia/include/codec"
  -I"$aosp_root/external/skia/include/core"
  -I"$aosp_root/external/skia/include/effects"
  -I"$aosp_root/external/skia/include/encode"
  -I"$aosp_root/external/skia/include/gpu"
  -I"$aosp_root/external/skia/include/pathops"
  -I"$aosp_root/external/skia/include/private"
  -I"$aosp_root/external/skia/include/utils"
  -I"$aosp_root/external/skia/src/core"
  -I"$aosp_root/external/vulkan-headers/include"
  -I"$aosp_root/libnativehelper-full/include"
  -I"$aosp_root/libnativehelper-full/include_platform"
  -I"$aosp_root/libnativehelper/header_only_include"
  -I"$aosp_root/libnativehelper/include_jni"
)

objects=()
compile_cached() {
  local label="$1" source="$2" object="$3"
  shift 3
  local meta="${object}.cmd"
  local command_file="${object}.command"
  local source_sha
  source_sha="$(shasum -a 256 "$source" | awk '{print $1}')"
  local -a command=("$cxx" "${flags[@]}" "$@" -MMD -MF "$object.d" -c "$source" -o "$object")
  local command_text
  command_text="$(printf '%q ' "${command[@]}")"
  local key
  # The source shadow tree is materialized from tracked patches, while the
  # object cache is shared across invocations. Include the complete patched
  # tree identity so header-only patch changes cannot reuse stale objects.
  key="$(printf '%s\n%s\n%s\n' "$patch_identity" "$source_sha" "$command_text" |
    shasum -a 256 | awk '{print $1}')"
  if [[ -f "$object" && -f "$meta" && -f "$command_file" && "$(<"$meta")" == "$key" ]]; then
    echo "hwui-static-foundation: cache $label"
    return
  fi
  echo "hwui-static-foundation: compile $label"
  "${command[@]}"
  printf '%s\n' "$key" > "$meta"
  printf '%s\n' "$command_text" > "${command_file}.tmp.$$"
  mv "${command_file}.tmp.$$" "$command_file"
}
for source in "${sources[@]}"; do
  object="$cache_dir/${source//\//_}.o"
  source_flags=()
  if [[ "$source" == DeferredLayerUpdater.cpp ||
        "$source" == AutoBackendTextureRelease.cpp ]]; then
    # This Android HWUI translation unit consumes Skia's AHardwareBuffer
    # extension while the shared Skia core itself remains a macOS target.
    source_flags+=( -DSK_BUILD_FOR_ANDROID -D__ANDROID_API__=36 )
  fi
  if [[ "$source" == renderthread/RenderThread.cpp ||
        "$source" == platform/host/renderthread/CacheManager.cpp ]]; then
    # These two units use Android HWUI's GPU-only CacheManager surface while
    # retaining Darwin's platform implementation beneath that interface.
    source_flags+=( -DDARWIN_ART_HWUI_GPU )
  fi
  compile_cached "$source" "$hwui_source/$source" "$object" \
    ${source_flags[@]+"${source_flags[@]}"}
  objects+=("$object")
done
generated_object="$cache_dir/HWUIProperties.sysprop.cpp.o"
compile_cached "generated HWUIProperties.sysprop.cpp" \
  "$generated_dir/source/HWUIProperties.sysprop.cpp" "$generated_object"
objects+=("$generated_object")

apex_objects=()
for source in "${apex_common_sources[@]}"; do
  object="$cache_dir/${source//\//_}.o"
  compile_cached "APEX-common $source" "$hwui_source/$source" "$object" \
    -I"$hwui_source/apex/include" -I"$hwui_source/jni"
  apex_objects+=("$object")
done

archive="$stage/libhwui-static-darwin.a"
"$ar" rcs "$archive" "${objects[@]}"
apex_archive="$stage/libandroid-graphics-apex-common-darwin.a"
"$ar" rcs "$apex_archive" "${apex_objects[@]}"
file "$archive" | grep -F 'current ar archive' >/dev/null
lipo -info "$archive" | grep -F 'architecture: arm64' >/dev/null
member_count="$("$ar" -t "$archive" | grep -v '^__\.SYMDEF' | wc -l | tr -d ' ')"
expected_member_count=$((HWUI_SOURCE_COUNT + 1))  # Generated HWUIProperties.sysprop.cpp.
[[ "$member_count" == "$expected_member_count" ]] || {
    echo "hwui-static-foundation: archive member count expected=$expected_member_count actual=$member_count" >&2
  exit 3
}
apex_member_count="$("$ar" -t "$apex_archive" | grep -v '^__\.SYMDEF' | wc -l | tr -d ' ')"
[[ "$apex_member_count" == 5 ]] || {
  echo "hwui-static-foundation: APEX-common member count expected=5 actual=$apex_member_count" >&2
  exit 3
}
definitions="$(nm -gUC "$archive")"
for symbol in \
  'android::SkiaCanvas::drawCircle(float, float, float, android::Paint const&)' \
  'android::uirenderer::renderthread::RenderThread::getInstance()' \
  'android::uirenderer::RenderNode::RenderNode('; do
  grep -F "$symbol" <<<"$definitions" >/dev/null || {
    echo "hwui-static-foundation: missing representative definition $symbol" >&2
    exit 3
  }
done

undefined_manifest="$output_dir/archive-undefined-symbols.txt"
nm -u "$archive" | awk '$1 ~ /^_/ { print $1 }' | sort -u > "$stage/archive-undefined-symbols.txt"
mv "$archive" "$output_dir/libhwui-static-darwin.a"
mv "$apex_archive" "$output_dir/libandroid-graphics-apex-common-darwin.a"
mv "$stage/archive-undefined-symbols.txt" "$undefined_manifest"
echo "hwui-static-foundation: objects=88 architecture=arm64"
echo "hwui-static-foundation: archive=$output_dir/libhwui-static-darwin.a"
echo "hwui-static-foundation: apex-common=$output_dir/libandroid-graphics-apex-common-darwin.a"
echo "hwui-static-foundation: unresolved-manifest=$undefined_manifest"
