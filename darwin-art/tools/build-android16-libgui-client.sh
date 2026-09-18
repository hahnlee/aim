#!/bin/bash
set -euo pipefail
script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/.." && pwd)"
bash "$script_dir/build-android16-surfaceflinger-core.sh" --archive-only
# ServiceManagement consumes the same guest filesystem and device-property
# owners as original NativeLoader. Rebuild and validate them before linking.
bash "$script_dir/build-android16-native-loader-policy.sh"
bash "$script_dir/build-android-owned-rust-providers.sh"
bash "$script_dir/test-original-looper.sh"
looper="$project_root/_build/looper-transport"
looper_rust="$project_root/target/debug/libdarwin_art_looper_transport.a"
android_owned_rust="$project_root/_build/android-owned-rust-providers/libandroid-owned-rust-providers.a"
policy="$project_root/_build/native-loader-policy"
providers="$project_root/_build/bionic-runtime-provider-closure"
shadow="$project_root/_build/surfaceflinger-core/work/frameworks-native"
generated="$project_root/_build/surfaceflinger-core/work/generated"
libhidl="$project_root/_aosp/android16-surfaceflinger-core/system-libhidl"
libfmq="$project_root/_aosp/android16-surfaceflinger-core/system-libfmq"
sdk_root="$(xcrun --sdk macosx --show-sdk-path)"
source "$script_dir/lib/surfaceflinger-compile-flags.sh"
# Every libgui consumer must see the same Looper layout as its object file.
flags=(-I"$looper/source/include" -I"$project_root/compat/looper" "${flags[@]}")
flags+=(-I"$project_root/_aosp/libnativehelper/include_jni")
out="$project_root/_build/libgui-client"
mkdir -p "$out"
stage="$(mktemp -d "$out/stage.XXXXXX")"
trap 'rm -rf -- "$stage"' EXIT
sysprop_cpp="$project_root/_build/hwui-static-deps/sysprop_cpp"
[[ -x "$sysprop_cpp" ]] || { echo "libgui-client: original sysprop_cpp required" >&2; exit 1; }
mkdir -p "$stage/generated/include" "$stage/generated/source" "$stage/generated/public"
"$sysprop_cpp" --header-dir "$stage/generated/include" \
  --source-dir "$stage/generated/source" --include-name LibGuiProperties.sysprop.h \
  --public-header-dir "$stage/generated/public" \
  "$shadow/libs/gui/sysprop/LibGuiProperties.sysprop"
flags+=(-I"$stage/generated/include")
libhwbinder="$project_root/_aosp/android16-surfaceflinger-core/system-libhwbinder"
hidl_gen="$project_root/_downloads/android16-surfaceflinger-core/tools/hidl-gen"
hidl_roots=(-r "android.hardware:$project_root/_aosp/android16-surfaceflinger-core/hardware-interfaces"
  -r "android.hidl:$libhidl/transport")
mkdir -p "$stage/hidl"
"$hidl_gen" -o "$stage/hidl" -Lc++-headers "${hidl_roots[@]}" \
  android.hidl.manager@1.1 android.hidl.manager@1.2 android.hidl.token@1.0
"$hidl_gen" -o "$stage/hidl" -Lc++-sources "${hidl_roots[@]}" \
  android.hardware.graphics.bufferqueue@1.0 android.hardware.graphics.bufferqueue@2.0
"$hidl_gen" -o "$stage/hidl" -Lc++-sources "${hidl_roots[@]}" \
  android.hidl.base@1.0 android.hidl.manager@1.0 android.hidl.manager@1.1 \
  android.hidl.manager@1.2 android.hidl.token@1.0
"$hidl_gen" -o "$stage/hidl" -Lc++-sources "${hidl_roots[@]}" \
  android.hardware.media@1.0 android.hardware.graphics.common@1.2
flags+=(-I"$stage/hidl" -I"$libhwbinder/include")
hardware="$project_root/_aosp/android16-surfaceflinger-core/hardware-interfaces"
"$hidl_gen" -o "$stage/hidl" -Lc++-headers "${hidl_roots[@]}" \
  android.hardware.graphics.allocator@2.0 android.hardware.graphics.allocator@3.0 \
  android.hardware.graphics.allocator@4.0 android.hardware.graphics.mapper@2.0 \
  android.hardware.graphics.mapper@2.1 android.hardware.graphics.mapper@3.0 \
  android.hardware.graphics.mapper@4.0
"$hidl_gen" -o "$stage/hidl" -Lc++-sources "${hidl_roots[@]}" \
  android.hardware.graphics.allocator@2.0 android.hardware.graphics.allocator@3.0 \
  android.hardware.graphics.allocator@4.0 android.hardware.graphics.mapper@4.0 \
  android.hardware.graphics.mapper@2.0 android.hardware.graphics.mapper@2.1 \
  android.hardware.graphics.mapper@3.0
allocator_api="$hardware/graphics/allocator/aidl/aidl_api/android.hardware.graphics.allocator/2"
allocator_aidl=()
while IFS= read -r source; do allocator_aidl+=("$source"); done \
  < <(find "$allocator_api/android" -name '*.aidl' | sort)
"$project_root/_downloads/android16-surfaceflinger-core/tools/aidl" \
  --lang=ndk --structured --stability=vintf --version=2 \
  --hash="$(tail -1 "$allocator_api/.hash")" --min_sdk_version=29 --omit_invocation \
  -I"$allocator_api" -I"$hardware/common/aidl" -I"$hardware/graphics/common/aidl" \
  -h "$stage/generated/include" -o "$stage/generated/source" "${allocator_aidl[@]}"
# This original libgui TU permits implicit sp<> construction; the SF frontend's
# stricter flag is not its Android.bp contract. Do not change original ownership
# conversions just to make those unrelated module flags compile.
sources=(SurfaceComposerClient TransactionState LayerStatePermissions IGraphicBufferProducer
  IGraphicBufferProducerFlattenables BatchBufferOps FrameTimestamps IProducerListener
  BufferQueueProducer SurfaceControl)
cp "$looper/Looper.o" "$stage/Looper.o"
xcrun clang++ "${flags[@]}" -c "$project_root/compat/looper/readiness_queue.cc" \
  -o "$stage/LooperReadinessQueue.o"
objects=("$stage/Looper.o" "$stage/LooperReadinessQueue.o")
source "$script_dir/lib/libgui-composer-sources.sh"
build_libgui_composer_sources
objects+=("${composer_objects[@]}")
ndk_sources=(ibinder.cpp libbinder.cpp parcel.cpp stability.cpp status.cpp service_manager.cpp)
ndk_objects=()
for source in "${ndk_sources[@]}"; do
  object="$stage/BinderNdk-${source%.cpp}.o"
  # Use the same original libbinder transport configuration as the SF archive.
  # Kernel Binder remains a separate platform integration requirement.
  xcrun clang++ "${flags[@]}" -UANDROID_UTILS_REF_BASE_DISABLE_IMPLICIT_CONSTRUCTION \
    -I"$shadow/libs/binder/ndk/include_platform" \
    -I"$project_root/_aosp/external/fmtlib/include" \
    -c "$shadow/libs/binder/ndk/$source" -o "$object"
  ndk_objects+=("$object")
  objects+=("$object")
done
buffer_native="$project_root/_aosp/android16-surfaceflinger-core/buffer-native"
buffer_core="$project_root/_aosp/android16-surfaceflinger-core/buffer-core"
binder_uapi="$project_root/_aosp/android16-surfaceflinger-core/binder-uapi/libc/kernel"
buffer_sources=("$buffer_native/libs/ui/GraphicBuffer.cpp"
  "$buffer_native/libs/nativewindow/AHardwareBuffer.cpp"
  "$buffer_native/libs/ui/GraphicBufferAllocator.cpp"
  "$buffer_native/libs/ui/GraphicBufferMapper.cpp"
  "$buffer_native/libs/ui/Gralloc.cpp"
  "$buffer_native/libs/ui/Gralloc2.cpp"
  "$buffer_native/libs/ui/Gralloc3.cpp"
  "$buffer_native/libs/ui/Gralloc4.cpp"
  "$buffer_native/libs/ui/Gralloc5.cpp"
  "$buffer_native/libs/gralloc/types/Gralloc4.cpp"
  "$hardware/common/support/NativeHandle.cpp"
  "$generated/aidl/src/android/hardware/common/NativeHandle.cpp"
  "$generated/aidl/src/android/hardware/graphics/common/ExtendableType.cpp"
  "$buffer_core/libgrallocusage/GrallocUsageConversion.cpp")
allocator_generated="$stage/generated/source/android/hardware/graphics/allocator"
[[ -f "$allocator_generated/IAllocator.cpp" ]] || {
  echo "libgui-client: generated allocator implementation missing" >&2; exit 1;
}
while IFS= read -r source; do buffer_sources+=("$source"); done \
  < <(find "$allocator_generated" -name '*.cpp' | sort)
buffer_objects=()
for source in "${buffer_sources[@]}"; do
  object="$stage/$(basename "${source%.cpp}").o"
  case "$source" in
    */libs/gralloc/types/*) object="$stage/Gralloc4Metadata.o" ;;
    */common/support/*) object="$stage/AidlNativeHandle.o" ;;
    "$generated/aidl/src/"*) object="$stage/CommonParcel-$(basename "${source%.cpp}").o" ;;
    "$allocator_generated/"*) object="$stage/AllocatorAidl-$(basename "${source%.cpp}").o" ;;
  esac
  buffer_flags=(-UANDROID_UTILS_REF_BASE_DISABLE_IMPLICIT_CONSTRUCTION)
  if [[ "$source" == */libs/nativewindow/AHardwareBuffer.cpp ]]; then
    buffer_flags+=('-D__INTRODUCED_IN(n)='
      -D__ANDROID_API__=36
      -include "$project_root/_aosp/android16-native-loader-policy/android/api-level.h"
      -I"$buffer_native/libs/nativewindow/include"
      -I"$buffer_native/libs/nativewindow/include-private")
  fi
  if [[ "$source" == */Gralloc5.cpp ]]; then
    buffer_flags+=('-D__INTRODUCED_IN(n)=' -D__ANDROID_API__=36
      -include "$project_root/_aosp/android16-native-loader-policy/android/api-level.h"
      -include "$project_root/compat/loader/native_loader_libdl.h")
  fi
  # libui does not adopt the SurfaceFlinger frontend's explicit-sp restriction.
  xcrun clang++ -I"$buffer_native/libs/nativewindow/include" \
    -I"$buffer_native/libs/ui/include" \
    -I"$buffer_native/libs/ui/include_types" -I"$buffer_core/libsync/include" "${flags[@]}" \
    -I"$buffer_native/libs/gralloc/types/include" \
    -I"$hardware/graphics/mapper/stable-c/include" \
    -I"$hardware/graphics/mapper/stable-c/implutils/include" \
    -I"$hardware/common/support/include" \
    -I"$shadow/libs/binder/ndk/include_platform" \
    -I"$project_root/_aosp/android16-surfaceflinger-core/system-core-vndksupport/libvndksupport/include" \
    -I"$binder_uapi/uapi" -I"$binder_uapi/uapi/asm-arm64" -I"$binder_uapi/android/uapi" \
    -I"$buffer_core/libgrallocusage/include" -I"$buffer_core/libsync/include" \
    "${buffer_flags[@]}" -c "$source" -o "$object"
  buffer_objects+=("$object")
  objects+=("$object")
done
nm -u "$stage/Gralloc5.o" > "$stage/gralloc5-imports.txt"
if rg -q ' _(dlopen|dlsym|dlclose|dlerror)$' "$stage/gralloc5-imports.txt"; then
  echo "libgui-client: Gralloc5 escaped guest loader boundary" >&2
  exit 1
fi
rg -q '_darwin_art_linker_dlsym$' "$stage/gralloc5-imports.txt"
rg -q '_AServiceManager_openDeclaredPassthroughHal$' "$stage/gralloc5-imports.txt"
echo "original Gralloc5: declared HAL service lookup and guest symbol loader imports PASS"
for unit in "${sources[@]}"; do
  xcrun clang++ "${flags[@]}" -UANDROID_UTILS_REF_BASE_DISABLE_IMPLICIT_CONSTRUCTION \
    -I"$project_root/_aosp/frameworks/native/opengl/include" \
    -c "$shadow/libs/gui/$unit.cpp" -o "$stage/$unit.o"
  objects+=("$stage/$unit.o")
done
xcrun clang++ "${flags[@]}" -c "$stage/generated/source/LibGuiProperties.sysprop.cpp" \
  -o "$stage/LibGuiProperties.o"
objects+=("$stage/LibGuiProperties.o")
for version in 1.0 2.0; do
  for listener in B2HProducerListener H2BProducerListener; do
    object="$stage/$listener-$version.o"
    xcrun clang++ "${flags[@]}" -UANDROID_UTILS_REF_BASE_DISABLE_IMPLICIT_CONSTRUCTION \
      -I"$project_root/_aosp/external/fmtlib/include" -c \
      "$shadow/libs/gui/bufferqueue/$version/$listener.cpp" -o "$object"
    objects+=("$object")
  done
  object="$stage/H2BGraphicBufferProducer-$version.o"
  xcrun clang++ "${flags[@]}" -UANDROID_UTILS_REF_BASE_DISABLE_IMPLICIT_CONSTRUCTION \
    -I"$project_root/_aosp/external/fmtlib/include" -c \
    "$shadow/libs/gui/bufferqueue/$version/H2BGraphicBufferProducer.cpp" -o "$object"
  objects+=("$object")
done
xcrun clang++ "${flags[@]}" -I"$project_root/_aosp/external/fmtlib/include" \
  -c "$libhidl/base/Status.cpp" -o "$stage/HidlStatus.o"
objects+=("$stage/HidlStatus.o")
xcrun clang++ "${flags[@]}" -UANDROID_UTILS_REF_BASE_DISABLE_IMPLICIT_CONSTRUCTION \
  -I"$project_root/_aosp/external/fmtlib/include" \
  -c "$libhidl/base/HidlSupport.cpp" -o "$stage/HidlSupport.o"
objects+=("$stage/HidlSupport.o")
xcrun clang++ "${flags[@]}" -UANDROID_UTILS_REF_BASE_DISABLE_IMPLICIT_CONSTRUCTION \
  -I"$project_root/_aosp/frameworks/native/opengl/include" \
  -I"$project_root/_aosp/external/fmtlib/include" \
  -c "$shadow/libs/gui/bufferqueue/2.0/types.cpp" -o "$stage/BufferQueueHidlTypes.o"
objects+=("$stage/BufferQueueHidlTypes.o")
hidl_generated_objects=()
for version in 1.0 2.0; do
  for interface in ProducerListener GraphicBufferProducer; do
    object="$stage/$interface-$version-generated.o"
    xcrun clang++ "${flags[@]}" -UANDROID_UTILS_REF_BASE_DISABLE_IMPLICIT_CONSTRUCTION \
      -I"$project_root/_aosp/external/fmtlib/include" -c \
      "$stage/hidl/android/hardware/graphics/bufferqueue/$version/${interface}All.cpp" -o "$object"
    hidl_generated_objects+=("$object")
    objects+=("$object")
  done
done
while IFS= read -r source; do
  relative="${source#"$stage/hidl/"}"
  object="$stage/${relative//\//_}.o"
  xcrun clang++ "${flags[@]}" -UANDROID_UTILS_REF_BASE_DISABLE_IMPLICIT_CONSTRUCTION \
    -I"$project_root/_aosp/external/fmtlib/include" -c "$source" -o "$object"
  hidl_generated_objects+=("$object")
  objects+=("$object")
done < <(find "$stage/hidl/android/hidl" "$stage/hidl/android/hardware/media" \
  "$stage/hidl/android/hardware/graphics/common" \
  "$stage/hidl/android/hardware/graphics/allocator" \
  "$stage/hidl/android/hardware/graphics/mapper" -name '*.cpp' | sort)
hidl_runtime_objects=()
hidl_runtime_sources=(base/TaskRunner.cpp base/HidlInternal.cpp transport/Static.cpp
  transport/HidlBinderSupport.cpp transport/HidlTransportUtils.cpp
  transport/HidlTransportSupport.cpp transport/HidlPassthroughSupport.cpp
  transport/ServiceManagement.cpp transport/token/1.0/utils/HybridInterface.cpp)
binder_uapi="$project_root/_aosp/android16-surfaceflinger-core/binder-uapi/libc/kernel"
api_headers="$project_root/_aosp/android16-native-loader-policy"
[[ "$(shasum -a 256 "$api_headers/android/api-level.h" | awk '{print $1}')" == \
  6bc855443a18af12edd4874bbdb176d77289ee54a3716ca2e212f3dd4c9bb81f ]]
cp "$libhidl/base/HidlInternal.cpp" "$stage/HidlInternal.cpp"
patch --batch --fuzz=0 -d "$stage" -p1 \
  < "$project_root/patches/libhidl/android-policy-on-darwin.patch"
cp "$libhidl/transport/ServiceManagement.cpp" "$stage/ServiceManagement.cpp"
patch --batch --fuzz=0 -d "$stage" -p1 \
  < "$project_root/patches/libhidl/guest-service-management.patch"
for source in "${hidl_runtime_sources[@]}"; do
  object="$stage/HidlRuntime-${source//\//_}.o"
  compile_source="$libhidl/$source"
  module_flags=("${flags[@]}")
  if [[ "$source" == base/HidlInternal.cpp ]]; then
    # Keep Android's API-dependent VNDK/APEX selection, not the host branch.
    compile_source="$stage/HidlInternal.cpp"
    module_flags+=(-DDARWIN_ART_ANDROID_HIDL_POLICY -D__ANDROID_API__=36 '-D__INTRODUCED_IN(n)='
      -I"$api_headers")
  fi
  if [[ "$source" == transport/HidlTransportSupport.cpp ]]; then
    module_flags+=(-I"$binder_uapi/uapi" -I"$binder_uapi/uapi/asm-arm64"
      -I"$binder_uapi/android/uapi")
  fi
  if [[ "$source" == transport/ServiceManagement.cpp ]]; then
    compile_source="$stage/ServiceManagement.cpp"
    # This TU includes original utils/Compat.h, which owns Darwin's lseek64.
    module_flags+=(-DDARWIN_ART_ANDROID_HIDL_POLICY -DDARWIN_ART_AOSP_COMPAT_LSEEK64
      '-D__INTRODUCED_IN(n)='
      -I"$project_root/compat" -I"$project_root/tools/bionic-fs-facade/include"
      -I"$project_root/tools/bionic-ioctl-facade/include"
      -I"$api_headers" -iquote "$libhidl/transport"
      -I"$project_root/_aosp/android16-surfaceflinger-core/system-core-vndksupport/libvndksupport/include"
      -include "$project_root/compat/loader/android_dlext_types.h"
      -include "$project_root/compat/loader/native_loader_libdl.h")
  fi
  xcrun clang++ "${module_flags[@]}" -UANDROID_UTILS_REF_BASE_DISABLE_IMPLICIT_CONSTRUCTION \
    -I"$project_root/_aosp/external/fmtlib/include" \
    -c "$compile_source" -o "$object"
  hidl_runtime_objects+=("$object")
  objects+=("$object")
done
hwbinder_objects=()
nm -u "$stage/HidlRuntime-base_HidlInternal.cpp.o" > "$stage/hidl-policy-imports.txt"
if ! rg -q '_android_get_device_api_level$' "$stage/hidl-policy-imports.txt" || \
   rg -q '_pthread_cond_clockwait$' "$stage/hidl-policy-imports.txt"; then
  echo "libgui-client: Android HIDL policy / Darwin pthread boundary violated" >&2
  exit 1
fi
echo "HIDL policy: original Android API lookup retained, no Linux condition-wait import PASS"
nm -u "$stage/HidlRuntime-transport_ServiceManagement.cpp.o" > "$stage/hidl-service-imports.txt"
if rg -q ' _(access|opendir|readdir|closedir|dlopen|dlsym|dlerror)$|basic_filebuf' \
    "$stage/hidl-service-imports.txt"; then
  echo "libgui-client: HIDL service lookup escaped guest file/loader boundary" >&2
  exit 1
fi
for symbol in GuestDirectory GuestFile ReadGuestConfig GuestLinkerAccess \
    darwin_art_linker_dlopen darwin_art_linker_dlsym android_load_sphal_library; do
  rg -q "$symbol" "$stage/hidl-service-imports.txt"
done
echo "original HIDL service lookup: guest files/directories and SP-HAL loader imports PASS"
xcrun clang++ -std=c++23 -arch arm64 -Wall -Wextra -Werror -Wno-macro-redefined \
  -I"$binder_uapi/uapi" -I"$binder_uapi/uapi/asm-arm64" -I"$binder_uapi/android/uapi" \
  "$project_root/tools/tests/binder-uapi-layout-test.cc" -o "$stage/uapi-test"
"$stage/uapi-test"
hwbinder_sources=(IInterface.cpp Parcel.cpp BpHwBinder.cpp Binder.cpp Static.cpp
  BufferedTextOutput.cpp TextOutput.cpp Debug.cpp ProcessState.cpp IPCThreadState.cpp Utils.cpp)
cp "$libhwbinder/Utils.cpp" "$stage/Utils.cpp"
patch --batch --fuzz=0 -d "$stage" -p1 \
  < "$project_root/patches/libhidl/hwbinder-guest-service-readiness.patch"
for source in "${hwbinder_sources[@]}"; do
  object="$stage/HwBinder-$source.o"
  compile_source="$libhwbinder/$source"
  module_flags=("${flags[@]}")
  if [[ "$source" == Utils.cpp ]]; then
    compile_source="$stage/Utils.cpp"
    module_flags+=(-DDARWIN_ART_ANDROID_HWBINDER_POLICY -D__BIONIC__ -iquote "$libhwbinder"
      -I"$project_root/compat" -I"$project_root/include"
      -I"$project_root/tools/bionic-fs-facade/include")
  fi
  xcrun clang++ "${module_flags[@]}" -UANDROID_UTILS_REF_BASE_DISABLE_IMPLICIT_CONSTRUCTION \
    -I"$binder_uapi/uapi" -I"$binder_uapi/uapi/asm-arm64" -I"$binder_uapi/android/uapi" \
    -I"$project_root/_aosp/external/fmtlib/include" -c "$compile_source" -o "$object"
  hwbinder_objects+=("$object")
  objects+=("$object")
done
nm -u "$stage/HwBinder-Utils.cpp.o" > "$stage/hwbinder-readiness-imports.txt"
if ! rg -q 'GuestLinkerAccess' "$stage/hwbinder-readiness-imports.txt" || \
   ! rg -q 'WaitForProperty' "$stage/hwbinder-readiness-imports.txt" || \
   rg -q ' _access$' "$stage/hwbinder-readiness-imports.txt"; then
  echo "libgui-client: hardware Binder readiness escaped guest filesystem/property policy" >&2
  exit 1
fi
echo "hardware Binder readiness: guest access and original property wait imports PASS"
vndksupport="$project_root/_aosp/android16-surfaceflinger-core/system-core-vndksupport/libvndksupport"
[[ "$(shasum -a 256 "$api_headers/android/dlext.h" | awk '{print $1}')" == \
  06f396c3624e580cc54283b1415e1e09919af317390104c44f9e217e37c94556 ]]
xcrun clang++ "${flags[@]}" '-D__INTRODUCED_IN(n)=' \
  -I"$api_headers" -I"$vndksupport/include/vndksupport" \
  -include "$project_root/compat/loader/android_dlext_types.h" \
  -include "$project_root/compat/loader/native_loader_libdl.h" \
  -c "$vndksupport/linker.cpp" -o "$stage/VndkSupport.o"
objects+=("$stage/VndkSupport.o")
nm -u "$stage/VndkSupport.o" > "$stage/vndksupport-imports.txt"
if rg -q ' _(dlopen|dlclose|dlerror)$' "$stage/vndksupport-imports.txt"; then
  echo "libgui-client: SP-HAL loader escaped to host dyld" >&2
  exit 1
fi
echo "original SP-HAL namespace policy compiled without host dyld imports PASS"
xcrun clang++ "${flags[@]}" -c "$project_root/tools/tests/hidl-support-ownership-test.cc" \
  -o "$stage/hidl-test.o"
xcrun clang++ -arch arm64 -Wl,-dead_strip "$stage/hidl-test.o" \
  "$stage/HidlSupport.o" "$stage/HidlStatus.o" \
  "$project_root/_build/libbase-foundation/libandroid-base-darwin.a" \
  "$project_root/_build/graphics-foundations/libcutils-darwin.a" \
  "$project_root/_build/graphics-foundations/libutils-darwin.a" \
  "$project_root/_build/graphics-foundations/liblog-darwin.a" -o "$stage/hidl-test"
"$stage/hidl-test"
xcrun clang++ "${flags[@]}" "$project_root/tools/tests/hidl-task-runner-test.cc" \
  "$stage/HidlRuntime-base_TaskRunner.cpp.o" \
  "$project_root/_build/graphics-foundations/libutils-darwin.a" \
  "$project_root/_build/graphics-foundations/liblog-darwin.a" -o "$stage/task-runner-test"
"$stage/task-runner-test"
xcrun clang++ -std=c++23 -Wall -Wextra -Werror -I"$project_root/compat" \
  "$project_root/tools/tests/commit-signal-test.cc" -o "$stage/commit-signal-test"
"$stage/commit-signal-test"
xcrun clang++ "${flags[@]}" -UANDROID_UTILS_REF_BASE_DISABLE_IMPLICIT_CONSTRUCTION \
  -I"$project_root/_aosp/frameworks/native/opengl/include" -c \
  "$project_root/tools/tests/libgui-transaction-state-test.cc" -o "$stage/state-test.o"
sf="$project_root/_build/surfaceflinger-core"
xcrun clang++ "${flags[@]}" "$project_root/tools/tests/binder-ndk-status-test.cc" \
  "$stage/BinderNdk-status.o" "$sf/libbinder-darwin.a" \
  "$project_root/_build/libbase-foundation/libandroid-base-darwin.a" \
  "$project_root/_build/graphics-foundations/libutils-darwin.a" \
  "$project_root/_build/graphics-foundations/libcutils-darwin.a" \
  "$project_root/_build/graphics-foundations/liblog-darwin.a" \
  -Wl,-dead_strip -o "$stage/ndk-status-test"
"$stage/ndk-status-test"
xcrun clang++ "${flags[@]}" -I"$shadow/libs/binder/ndk/include_platform" \
  "$project_root/tools/tests/binder-ndk-rpc-test.cc" \
  "$stage/BinderNdk-ibinder.o" "$stage/BinderNdk-libbinder.o" \
  "$stage/BinderNdk-parcel.o" "$stage/BinderNdk-status.o" "$stage/BinderNdk-stability.o" \
  "$sf/libbinder-darwin.a" \
  "$project_root/_build/libbase-foundation/libandroid-base-darwin.a" \
  "$project_root/_build/graphics-foundations/libutils-darwin.a" \
  "$project_root/_build/graphics-foundations/libcutils-darwin.a" \
  "$project_root/_build/graphics-foundations/liblog-darwin.a" \
  -Wl,-dead_strip -o "$stage/ndk-rpc-test"
"$stage/ndk-rpc-test"
xcrun clang++ "${flags[@]}" -c "$project_root/tools/tests/producer-listener-parcel-test.cc" \
  -o "$stage/listener-test.o"
xcrun clang++ -arch arm64 -Wl,-dead_strip "$stage/listener-test.o" \
  "$stage/IProducerListener.o" "$stage/BufferQueueProducer.o" \
  "$stage/HidlSupport.o" "$stage/HidlStatus.o" \
  "$sf/libbinder-darwin.a" \
  "$project_root/_build/libbase-foundation/libandroid-base-darwin.a" \
  "$project_root/_build/graphics-foundations/libutils-darwin.a" \
  "$project_root/_build/graphics-foundations/libcutils-darwin.a" \
  "$project_root/_build/graphics-foundations/liblog-darwin.a" -o "$stage/listener-test"
"$stage/listener-test"
xcrun clang++ "${flags[@]}" -c \
  "$project_root/tools/tests/buffer-release-message-test.cc" -o "$stage/message-test.o"
xcrun clang++ -arch arm64 -Wl,-dead_strip "$stage/message-test.o" \
  "$sf/libgui-transaction-darwin.a" "$sf/libbinder-darwin.a" "$sf/libui-fence-darwin.a" \
  "$project_root/_build/ui-types-foundation/libui-types.a" \
  "$project_root/_build/graphics-foundations/libutils-darwin.a" \
  "$project_root/_build/graphics-foundations/libcutils-darwin.a" \
  "$project_root/_build/graphics-foundations/liblog-darwin.a" -o "$stage/message-test"
"$stage/message-test"
xcrun clang++ "${flags[@]}" -c \
  "$project_root/tools/tests/buffer-release-channel-test.cc" -o "$stage/channel-test.o"
xcrun clang++ -arch arm64 -Wl,-dead_strip "$stage/channel-test.o" \
  "$sf/libgui-transaction-darwin.a" "$sf/libbinder-darwin.a" "$sf/libui-fence-darwin.a" \
  "$project_root/_build/ui-types-foundation/libui-types.a" \
  "$project_root/_build/graphics-foundations/libutils-darwin.a" \
  "$project_root/_build/graphics-foundations/libcutils-darwin.a" \
  "$project_root/_build/graphics-foundations/liblog-darwin.a" -o "$stage/channel-test"
"$stage/channel-test"
xcrun clang -std=c11 -I"$project_root/tools/bionic-errno-tls/include" \
  -I"$project_root/tools/bionic-errno-tls/generated" \
  -c "$project_root/tools/bionic-errno-tls/src/errno_tls.c" -o "$stage/guest-errno.o"
xcrun clang++ -arch arm64 -Wl,-dead_strip "$stage/state-test.o" "$stage/TransactionState.o" \
  "$stage/SurfaceControl.o" "$stage/SurfaceComposerClient.o" \
  "${composer_objects[@]}" \
  "$stage/Looper.o" "$stage/LooperReadinessQueue.o" \
  "$stage/IGraphicBufferProducer.o" \
  "$stage/IGraphicBufferProducerFlattenables.o" "$stage/BatchBufferOps.o" "$stage/FrameTimestamps.o" \
  "$stage/LibGuiProperties.o" \
  "$stage/H2BGraphicBufferProducer-1.0.o" "$stage/H2BGraphicBufferProducer-2.0.o" \
  "$stage/HidlStatus.o" \
  "$stage/HidlSupport.o" "$stage/BufferQueueHidlTypes.o" \
  "$stage/IProducerListener.o" \
  "$stage/BufferQueueProducer.o" \
  "$stage/B2HProducerListener-1.0.o" "$stage/B2HProducerListener-2.0.o" \
  "$stage/H2BProducerListener-1.0.o" "$stage/H2BProducerListener-2.0.o" \
  "${hidl_generated_objects[@]}" \
  "${hidl_runtime_objects[@]}" \
  "${hwbinder_objects[@]}" \
  "$stage/VndkSupport.o" \
  "${buffer_objects[@]}" \
  "${ndk_objects[@]}" \
  "$sf/libgui-transaction-darwin.a" "$sf/libbinder-darwin.a" "$sf/libui-fence-darwin.a" \
  "$project_root/_build/ui-types-foundation/libui-types.a" \
  "$project_root/_build/graphics-foundations/libutils-darwin.a" \
  "$project_root/_build/graphics-foundations/libcutils-darwin.a" \
  "$policy/device_properties.o" "$stage/guest-errno.o" \
  "$policy/libnative-loader-policy-darwin.a" \
  "$project_root/_build/libbase-foundation/libandroid-base-darwin.a" \
  "$providers/libdarwin-art-bionic-native-providers.a" \
  "$android_owned_rust" \
  "$looper_rust" \
  "$project_root/_build/graphics-foundations/liblog-darwin.a" \
  -framework Security -lresolv -o "$stage/state-test"
nm -a "$stage/state-test" > "$stage/state-test-symbols.txt"
if rg -q 'g_properties' "$stage/state-test-symbols.txt"; then
  echo "libgui-client: host property store leaked into Android service policy" >&2
  exit 1
fi
for symbol in get find read_callback wait serial area_serial; do
  rg -q "_darwin_art_bionic___system_property_${symbol}$" "$stage/state-test-symbols.txt"
done
"$stage/state-test"
xcrun ar rcs "$stage/libgui-client-darwin.a" "${objects[@]}"
nm -u "${objects[@]}" > "$stage/undefined-symbols.txt"
if rg -q '_sem_(init|post|destroy|clockwait)$' "$stage/undefined-symbols.txt"; then
  echo "libgui-client: unsupported Darwin semaphore import remains" >&2
  exit 1
fi
for unit in "${sources[@]}"; do
  shasum -a 256 "$shadow/libs/gui/$unit.cpp"
done > "$stage/source-identity.txt"
shasum -a 256 "${composer_sources[@]}" "$script_dir/lib/libgui-composer-sources.sh" \
  >> "$stage/source-identity.txt"
shasum -a 256 "$looper/source/Looper.cpp" "$looper/source/include/utils/Looper.h" \
  "$project_root/patches/libutils/darwin-looper-platform.patch" \
  "$project_root/compat/looper/readiness_queue.cc" \
  "$project_root/compat/looper/readiness_queue.h" "$project_root/compat/looper/readiness.h" \
  "$looper_rust" >> "$stage/source-identity.txt"
for source in "${ndk_sources[@]}"; do
  shasum -a 256 "$shadow/libs/binder/ndk/$source"
done >> "$stage/source-identity.txt"
shasum -a 256 "${buffer_sources[@]}" \
  "$project_root/_aosp/android16-surfaceflinger-core/buffer-identity" \
  "$project_root/_aosp/android16-surfaceflinger-core/binder-uapi-identity" \
  "$allocator_api/.hash" "${allocator_aidl[@]}" \
  >> "$stage/source-identity.txt"
shasum -a 256 "$sysprop_cpp" "$shadow/libs/gui/sysprop/LibGuiProperties.sysprop" \
  "$stage/generated/source/LibGuiProperties.sysprop.cpp" >> "$stage/source-identity.txt"
shasum -a 256 "$libhidl/base/Status.cpp" \
  "$libhidl/base/HidlSupport.cpp" "$shadow/libs/gui/bufferqueue/2.0/types.cpp" \
  "$shadow/libs/gui/bufferqueue/1.0/H2BGraphicBufferProducer.cpp" \
  "$shadow/libs/gui/bufferqueue/2.0/H2BGraphicBufferProducer.cpp" >> "$stage/source-identity.txt"
for version in 1.0 2.0; do
  shasum -a 256 "$shadow/libs/gui/bufferqueue/$version/B2HProducerListener.cpp" \
    "$shadow/libs/gui/bufferqueue/$version/H2BProducerListener.cpp"
done >> "$stage/source-identity.txt"
shasum -a 256 "$hidl_gen" >> "$stage/source-identity.txt"
shasum -a 256 "$project_root/upstream/android16-surfaceflinger-core.lock" >> "$stage/source-identity.txt"
shasum -a 256 "$policy/device_properties.o" \
  "$policy/libnative-loader-policy-darwin.a" \
  "$project_root/_build/libbase-foundation/libandroid-base-darwin.a" \
  "$providers/libdarwin-art-bionic-native-providers.a" \
  "$android_owned_rust" \
  "$project_root/tools/bionic-errno-tls/src/errno_tls.c" >> "$stage/source-identity.txt"
while IFS= read -r source; do shasum -a 256 "$source"; done \
  < <(find "$stage/hidl/android/hidl" "$stage/hidl/android/hardware/media" \
    "$stage/hidl/android/hardware/graphics/common" \
    "$stage/hidl/android/hardware/graphics/allocator" \
    "$stage/hidl/android/hardware/graphics/mapper" -name '*.cpp' | sort) >> "$stage/source-identity.txt"
for source in "${hidl_runtime_sources[@]}"; do
  shasum -a 256 "$libhidl/$source"
done >> "$stage/source-identity.txt"
shasum -a 256 "$api_headers/android/api-level.h" >> "$stage/source-identity.txt"
shasum -a 256 "$vndksupport/linker.cpp" "$vndksupport/include/vndksupport/linker.h" \
  "$api_headers/android/dlext.h" "$project_root/compat/loader/android_dlext_types.h" \
  "$project_root/compat/loader/native_loader_libdl.h" >> "$stage/source-identity.txt"
shasum -a 256 "$project_root/patches/libhidl/android-policy-on-darwin.patch" \
  "$stage/HidlInternal.cpp" >> "$stage/source-identity.txt"
shasum -a 256 "$project_root/patches/libhidl/guest-service-management.patch" \
  "$stage/ServiceManagement.cpp" >> "$stage/source-identity.txt"
for source in "${hwbinder_sources[@]}"; do
  shasum -a 256 "$libhwbinder/$source"
done >> "$stage/source-identity.txt"
shasum -a 256 "$project_root/patches/libhidl/hwbinder-guest-service-readiness.patch" \
  "$stage/Utils.cpp" "$project_root/compat/filesystem/linker_config_fs.h" \
  >> "$stage/source-identity.txt"
for version in 1.0 2.0; do
  shasum -a 256 "$stage/hidl/android/hardware/graphics/bufferqueue/$version/ProducerListenerAll.cpp" \
    "$stage/hidl/android/hardware/graphics/bufferqueue/$version/GraphicBufferProducerAll.cpp"
done >> "$stage/source-identity.txt"
shasum -a 256 \
  "$project_root/compat/surfaceflinger/commit_signal.h" \
  "$project_root/patches/frameworks-native/0002-darwin-surface-commit-wait.patch" \
  "$script_dir/lib/surfaceflinger-compile-flags.sh" >> "$stage/source-identity.txt"
for object in "${objects[@]}"; do mv "$object" "$out/"; done
mv "$stage/libgui-client-darwin.a" "$out/libgui-client-darwin.a"
mv "$stage/undefined-symbols.txt" "$out/undefined-symbols.txt"
mv "$stage/source-identity.txt" "$out/source-identity.txt"
echo "libgui-client: original client/state/permission objects compiled; final provider closure and runtime installation REQUIRED"
