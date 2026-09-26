#!/bin/bash
set -euo pipefail

mode="${1:---archive-and-test}"
[[ $# -le 1 && ( "$mode" == --archive-only || "$mode" == --archive-and-test ) ]] || {
  echo 'usage: build-android16-surfaceflinger-core.sh [--archive-only|--archive-and-test]' >&2
  exit 2
}

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/.." && pwd)"
source_root="$project_root/_aosp/android16-surfaceflinger-core"
frameworks_native="$source_root/frameworks-native"
hardware_interfaces="$source_root/hardware-interfaces"
libhidl="$source_root/system-libhidl"
libfmq="$source_root/system-libfmq"
uapi="$source_root/binder-uapi/libc/kernel"
tool_root="$project_root/_downloads/android16-surfaceflinger-core/tools"
output_root="$project_root/_build/surfaceflinger-core"
patch_file="$project_root/patches/frameworks-native/0001-darwin-surfaceflinger-core.patch"
commit_patch="$project_root/patches/frameworks-native/0002-darwin-surface-commit-wait.patch"
release_patch="$project_root/patches/frameworks-native/0003-buffer-release-message-length.patch"
transport_patch="$project_root/patches/frameworks-native/0004-darwin-release-record-transport.patch"
trigger_patch="$project_root/patches/frameworks-native/0005-darwin-binder-trigger-poll.patch"
rpc_identity_patch="$project_root/patches/frameworks-native/0006-darwin-binder-rpc-peer-identity.patch"
rpc_death_log_patch="$project_root/patches/frameworks-native/0007-rpc-binder-death-log-handle.patch"
platform_syscalls_patch="$project_root/patches/frameworks-native/0008-darwin-binder-platform-syscalls.patch"
fd_namespace_patch="$project_root/patches/frameworks-native/0009-darwin-binder-fd-namespace.patch"

# shellcheck disable=SC1090
source "$project_root/upstream/android16-surfaceflinger-core.lock"

"$script_dir/sync-android16-surfaceflinger-core.sh"
"$script_dir/sync-android16-hostgraphics.sh" >/dev/null

fail_build() {
  echo "surfaceflinger-core-build: $1" >&2
  exit 2
}

for input in \
  "$frameworks_native/services/surfaceflinger/FrontEnd/TransactionHandler.cpp" \
  "$frameworks_native/services/surfaceflinger/FrontEnd/LayerLifecycleManager.cpp" \
  "$frameworks_native/services/surfaceflinger/FrontEnd/LayerHierarchy.cpp" \
  "$frameworks_native/services/surfaceflinger/FrontEnd/RequestedLayerState.cpp" \
  "$frameworks_native/services/surfaceflinger/FrontEnd/LayerCreationArgs.cpp" \
  "$frameworks_native/libs/gui/libgui_flags.aconfig" \
  "$frameworks_native/libs/input/android/os/IInputConstants.aidl" \
  "$hardware_interfaces/graphics/common/aidl/Android.bp" \
  "$libhidl/base/include/hidl/HidlSupport.h" \
  "$libfmq/base/fmq/MQDescriptorBase.h" \
  "$tool_root/aidl" "$tool_root/hidl-gen" "$tool_root/aconfig" \
  "$patch_file" "$commit_patch" "$release_patch" "$transport_patch" "$trigger_patch" \
  "$rpc_identity_patch" "$rpc_death_log_patch" "$platform_syscalls_patch" \
  "$fd_namespace_patch"; do
  [[ -e "$input" ]] || fail_build "missing input $input"
done

[[ "$(shasum -a 256 "$commit_patch" | awk '{print $1}')" == "$DARWIN_COMMIT_WAIT_PATCH_SHA256" ]] || \
  fail_build "commit wait patch identity mismatch"
[[ "$(shasum -a 256 "$release_patch" | awk '{print $1}')" == "$BUFFER_RELEASE_LENGTH_PATCH_SHA256" ]] || \
  fail_build "buffer release length patch identity mismatch"
[[ "$(shasum -a 256 "$transport_patch" | awk '{print $1}')" == "$DARWIN_RELEASE_TRANSPORT_PATCH_SHA256" ]] || \
  fail_build "release transport patch identity mismatch"
[[ "$(shasum -a 256 "$rpc_identity_patch" | awk '{print $1}')" == "$DARWIN_BINDER_RPC_PEER_IDENTITY_PATCH_SHA256" ]] || \
  fail_build "Binder RPC peer identity patch mismatch"
[[ "$(shasum -a 256 "$rpc_death_log_patch" | awk '{print $1}')" == "$DARWIN_BINDER_RPC_DEATH_LOG_PATCH_SHA256" ]] || \
  fail_build "Binder RPC death-log patch mismatch"
[[ "$(shasum -a 256 "$platform_syscalls_patch" | awk '{print $1}')" == "$DARWIN_BINDER_PLATFORM_SYSCALLS_PATCH_SHA256" ]] || \
  fail_build "Binder platform-syscall patch mismatch"
[[ "$(shasum -a 256 "$fd_namespace_patch" | awk '{print $1}')" == "$DARWIN_BINDER_FD_NAMESPACE_PATCH_SHA256" ]] || \
  fail_build "Binder FD namespace patch mismatch"

cxx="$(xcrun --find clang++)"
ar="$(xcrun --find ar)"
sdk_root="$(xcrun --sdk macosx --show-sdk-path)"
mkdir -p "$output_root"
workspace="$output_root/work"
shadow="$workspace/frameworks-native"
generated="$workspace/generated"
generation_identity="$(printf '%s\n%s\n%s\n%s\n' \
  "$FRAMEWORKS_NATIVE_REVISION" "$HARDWARE_INTERFACES_REVISION" \
  "$SYSTEM_LIBHIDL_REVISION" "$SYSTEM_LIBFMQ_REVISION"; \
  shasum -a 256 "$patch_file" "$tool_root/aidl" "$tool_root/hidl-gen" \
    "$tool_root/aconfig" "$commit_patch" "$release_patch" "$transport_patch" "$trigger_patch" \
    "$rpc_identity_patch" "$rpc_death_log_patch" "$platform_syscalls_patch" \
    "$fd_namespace_patch" \
    "$project_root/compat/surfaceflinger/commit_signal.h"; \
  printf '%s\n' 'input-aidl:InputConfig+IInputConstants-v1')"
generation_identity="$(printf '%s' "$generation_identity" | shasum -a 256 | awk '{print $1}')"

if [[ ! -f "$workspace/generation-identity" || \
      "$(<"$workspace/generation-identity")" != "$generation_identity" ]]; then
  rm -rf "$workspace"
  mkdir -p "$workspace"
  ditto "$frameworks_native" "$shadow"
  patch -s -d "$shadow" -p1 < "$patch_file"
  patch -s -d "$shadow" -p1 < "$commit_patch"
  patch -s -d "$shadow" -p1 < "$release_patch"
  patch -s -d "$shadow" -p1 < "$transport_patch"
  patch --batch --fuzz=0 -s -d "$shadow" -p1 < "$trigger_patch"
  patch --batch --fuzz=0 -s -d "$shadow" -p1 < "$rpc_identity_patch"
  patch --batch --fuzz=0 -s -d "$shadow" -p1 < "$rpc_death_log_patch"
  patch --batch --fuzz=0 -s -d "$shadow" -p1 < "$platform_syscalls_patch"
  patch --batch --fuzz=0 -s -d "$shadow" -p1 < "$fd_namespace_patch"

  mkdir -p "$generated/aidl/include" "$generated/aidl/src" \
  "$generated/gui/include" "$generated/gui/src" \
  "$generated/input/include" "$generated/input/src" \
  "$generated/platform/include" "$generated/platform/src" \
  "$generated/hidl" "$generated/aconfig"

common_aidl=()
while IFS= read -r aidl_file; do
  common_aidl+=("$aidl_file")
done < <(find \
  "$hardware_interfaces/common/aidl/android" \
  "$hardware_interfaces/graphics/common/aidl/android" \
  -name '*.aidl' | sort)
"$tool_root/aidl" --lang=ndk --structured --stability=vintf \
  --min_sdk_version=29 --omit_invocation \
  -I"$hardware_interfaces/common/aidl" \
  -I"$hardware_interfaces/graphics/common/aidl" \
  -h "$generated/aidl/include" -o "$generated/aidl/src" \
  "${common_aidl[@]}"

for type in ColorMode Composition RenderIntent; do
  "$tool_root/aidl" --lang=ndk --structured --stability=vintf \
    --min_sdk_version=29 --omit_invocation \
    -I"$hardware_interfaces/common/aidl" \
    -I"$hardware_interfaces/graphics/common/aidl" \
    -I"$hardware_interfaces/graphics/composer/aidl" \
    -h "$generated/aidl/include" -o "$generated/aidl/src" \
    "$hardware_interfaces/graphics/composer/aidl/android/hardware/graphics/composer3/$type.aidl"
done

gui_aidl=()
while IFS= read -r aidl_file; do
  gui_aidl+=("$aidl_file")
done < <(find \
  "$shadow/libs/gui/aidl/android" "$shadow/libs/gui/android" \
  -name '*.aidl' | sort)
"$tool_root/aidl" --lang=cpp --min_sdk_version=29 --omit_invocation \
  -I"$shadow/libs/gui" -I"$shadow/libs/gui/aidl" \
  -I"$hardware_interfaces/common/aidl" \
  -I"$hardware_interfaces/graphics/common/aidl" \
  -I"$hardware_interfaces/graphics/composer/aidl" \
  -h "$generated/gui/include" -o "$generated/gui/src" \
  "${gui_aidl[@]}"

"$tool_root/aidl" --lang=cpp --min_sdk_version=29 --omit_invocation \
  -I"$shadow/libs/input" \
  -h "$generated/input/include" -o "$generated/input/src" \
  "$shadow/libs/input/android/os/InputConfig.aidl" \
  "$shadow/libs/input/android/os/IInputConstants.aidl"
"$tool_root/aidl" --lang=cpp --min_sdk_version=29 --omit_invocation \
  -I"$shadow/aidl/gui" \
  -h "$generated/platform/include" -o "$generated/platform/src" \
  "$shadow/aidl/gui/android/view/LayerMetadataKey.aidl"

hidl_roots=(-r "android.hardware:$hardware_interfaces" -r "android.hidl:$libhidl/transport")
"$tool_root/hidl-gen" -o "$generated/hidl" -Lc++-headers "${hidl_roots[@]}" \
  android.hidl.base@1.0 android.hidl.manager@1.0 \
  android.hardware.graphics.common@1.0 \
  android.hardware.graphics.common@1.1 \
  android.hardware.graphics.common@1.2 \
  android.hardware.media@1.0 \
  android.hardware.graphics.bufferqueue@1.0 \
  android.hardware.graphics.bufferqueue@2.0

"$tool_root/aconfig" create-cache \
  --package com.android.graphics.libgui.flags --container system \
  --declarations "$shadow/libs/gui/libgui_flags.aconfig" \
  --default-permission read_only --allow-read-write false \
  --cache "$generated/aconfig/libgui.cache"
"$tool_root/aconfig" create-cpp-lib \
  --cache "$generated/aconfig/libgui.cache" \
  --out "$generated/aconfig" --mode force-read-only
  printf '%s\n' "$generation_identity" > "$workspace/generation-identity"
fi

artifact_stage="$(mktemp -d "$output_root/artifact.XXXXXX")"
trap 'rm -rf "$artifact_stage"' EXIT

source "$script_dir/lib/surfaceflinger-compile-flags.sh"

object_root="$output_root/objects"
build_identity="$(printf '%s\n%s\n' "$generation_identity" \
  "$("$cxx" --version | head -1)")"
build_identity="$(printf '%s' "$build_identity" | shasum -a 256 | awk '{print $1}')"
if [[ ! -f "$object_root/build-identity" || \
      "$(<"$object_root/build-identity")" != "$build_identity" ]]; then
  rm -rf "$object_root"
  mkdir -p "$object_root"
  printf '%s\n' "$build_identity" > "$object_root/build-identity"
fi

compile_object() {
  local object="$1"
  shift
  local fingerprint_file="$object.fingerprint"
  local fingerprint
  # Sources name only the .cpp; the Darwin shim headers they include
  # (compat/binder) must also invalidate the object.
  fingerprint="$({ printf '%q\n' "$cxx" "$@"; \
    for input in "$@"; do \
      if [[ -f "$input" ]]; then shasum -a 256 "$input"; fi; \
    done; \
    shasum -a 256 "$project_root"/compat/binder/*.h; \
  } | shasum -a 256 | awk '{print $1}')"
  if [[ -f "$object" && -f "$fingerprint_file" && \
        "$(<"$fingerprint_file")" == "$fingerprint" ]]; then
    return
  fi
  local temporary="$object.tmp.$$"
  "$cxx" "$@" -o "$temporary"
  mv "$temporary" "$object"
  printf '%s\n' "$fingerprint" > "$fingerprint_file"
}

handler_object="$object_root/TransactionHandler.o"
lifecycle_object="$object_root/LayerLifecycleManager.o"
hierarchy_object="$object_root/LayerHierarchy.o"
requested_layer_object="$object_root/RequestedLayerState.o"
creation_args_object="$object_root/LayerCreationArgs.o"
transaction_bridge_object="$object_root/transaction-bridge.o"
layer_state_factory_object="$object_root/layer-state-factory.o"
compile_object "$handler_object" "${flags[@]}" -c \
  "$shadow/services/surfaceflinger/FrontEnd/TransactionHandler.cpp"
compile_object "$lifecycle_object" "${flags[@]}" -c \
  "$shadow/services/surfaceflinger/FrontEnd/LayerLifecycleManager.cpp"
compile_object "$hierarchy_object" "${flags[@]}" -c \
  "$shadow/services/surfaceflinger/FrontEnd/LayerHierarchy.cpp"
compile_object "$requested_layer_object" "${flags[@]}" -c \
  "$shadow/services/surfaceflinger/FrontEnd/RequestedLayerState.cpp"
compile_object "$creation_args_object" "${flags[@]}" -c \
  "$shadow/services/surfaceflinger/FrontEnd/LayerCreationArgs.cpp"
compile_object "$transaction_bridge_object" "${flags[@]}" -c \
  -include "$project_root/compat/surfaceflinger/transaction_bridge.h" \
  "$project_root/compat/surfaceflinger/transaction_bridge.cc"
compile_object "$layer_state_factory_object" "${flags[@]}" -c \
  "$project_root/compat/surfaceflinger/layer_state_factory.cc"

gui_runtime_objects=()
for generated_source in \
  android/gui/BorderSettings.cpp \
  android/gui/EdgeExtensionParameters.cpp \
  android/gui/FocusRequest.cpp \
  android/gui/FrameTimelineInfo.cpp \
  android/gui/InputApplicationInfo.cpp \
  android/gui/IWindowInfosReportedListener.cpp \
  android/gui/LayerMetadata.cpp \
  android/gui/TrustedPresentationThresholds.cpp; do
  object="$object_root/generated-$(basename "${generated_source%.cpp}").o"
  compile_object "$object" "${flags[@]}" -c "$generated/gui/src/$generated_source"
  gui_runtime_objects+=("$object")
done

binder_objects=()
binder_sources=(
  IPCThreadState.cpp
  ProcessState.cpp
  Static.cpp
  BufferedTextOutput.cpp
  Binder.cpp
  BpBinder.cpp
  Debug.cpp
  FdTrigger.cpp
  IInterface.cpp
  IResultReceiver.cpp
  Parcel.cpp
  ParcelFileDescriptor.cpp
  RpcSession.cpp
  RpcServer.cpp
  RpcState.cpp
  RpcTransportRaw.cpp
  Stability.cpp
  Status.cpp
  TextOutput.cpp
  Utils.cpp
  file.cpp
  OS_unix_base.cpp
  RecordedTransaction.cpp
)
for binder_source in "${binder_sources[@]}"; do
  object="$object_root/binder-${binder_source%.cpp}.o"
  binder_extra_flags=()
  if [[ "$binder_source" == RpcTransportRaw.cpp ]]; then
    # Original libbase supplies the platform TEMP_FAILURE_RETRY macro on Darwin.
    binder_extra_flags=(-include android-base/macros.h)
  fi
  if [[ "$binder_source" == RpcSession.cpp || "$binder_source" == RpcServer.cpp || \
        "$binder_source" == RpcState.cpp || "$binder_source" == ProcessState.cpp || \
        "$binder_source" == IPCThreadState.cpp || "$binder_source" == Parcel.cpp || \
        "$binder_source" == OS_unix_base.cpp ]]; then
    binder_extra_flags+=( -I"$project_root/compat/binder" )
  fi
  compile_object "$object" "${flags[@]}" \
    ${binder_extra_flags[@]+"${binder_extra_flags[@]}"} \
    -DBUILDING_LIBBINDER \
    -DBINDER_WITH_KERNEL_IPC \
    -DBINDER_ENABLE_LIBLOG_ASSERT \
    -DBINDER_DISABLE_BLOB \
    -I"$uapi/uapi" -I"$uapi/uapi/asm-arm64" -I"$uapi/android/uapi" \
    -include "$project_root/compat/binder/device_uapi.h" \
    -include "$project_root/compat/surfaceflinger/binder_socket_darwin.h" \
    -c "$shadow/libs/binder/$binder_source"
  binder_objects+=("$object")
done
binder_os_object="$object_root/binder-os-darwin.o"
compile_object "$binder_os_object" "${flags[@]}" \
  -DBUILDING_LIBBINDER \
  -DBINDER_WITH_KERNEL_IPC \
  -DBINDER_ENABLE_LIBLOG_ASSERT \
  -DBINDER_DISABLE_BLOB \
  -include "$project_root/compat/surfaceflinger/binder_socket_darwin.h" \
  -I"$shadow/libs/binder" \
  -c "$project_root/compat/surfaceflinger/binder_os_darwin.cc"
binder_objects+=("$binder_os_object")

fence_object="$object_root/ui-Fence.o"
compile_object "$fence_object" "${flags[@]}" \
  -UANDROID_UTILS_REF_BASE_DISABLE_IMPLICIT_CONSTRUCTION -c \
  "$project_root/_aosp/frameworks/native/libs/ui/Fence.cpp"
fence_sync_object="$object_root/fence-sync-darwin.o"
compile_object "$fence_sync_object" "${flags[@]}" -c \
  "$project_root/compat/surfaceflinger/fence_sync_darwin.cc"
fence_time_object="$object_root/ui-FenceTime.o"
compile_object "$fence_time_object" "${flags[@]}" -c \
  -UANDROID_UTILS_REF_BASE_DISABLE_IMPLICIT_CONSTRUCTION \
  "$project_root/_aosp/frameworks/native/libs/ui/FenceTime.cpp"
picture_profile_object="$object_root/ui-PictureProfileHandle.o"
compile_object "$picture_profile_object" "${flags[@]}" -c \
  "$project_root/_aosp/frameworks/native/libs/ui/PictureProfileHandle.cpp"
gui_runtime_objects+=("$picture_profile_object")
aconfig_runtime_object="$object_root/generated-libgui-aconfig.o"
compile_object "$aconfig_runtime_object" "${flags[@]}" -c \
  "$generated/aconfig/com_android_graphics_libgui_flags.cc"
gui_runtime_objects+=("$aconfig_runtime_object")
for gui_source in LayerMetadata.cpp LayerState.cpp ITransactionCompletedListener.cpp HdrMetadata.cpp WindowInfo.cpp DisplayLuts.cpp BufferReleaseChannel.cpp; do
  object="$object_root/gui-${gui_source%.cpp}.o"
  compile_object "$object" "${flags[@]}" \
    -I"$project_root/_aosp/external/fmtlib/include" -c "$shadow/libs/gui/$gui_source"
  gui_runtime_objects+=("$object")
done

archive="$artifact_stage/libsurfaceflinger-frontend-darwin.a"
release_transport_object="$object_root/release-record-transport.o"
compile_object "$release_transport_object" "${flags[@]}" -c \
  "$project_root/compat/surfaceflinger/release_record_transport.cc"
gui_runtime_objects+=("$release_transport_object")
"$ar" rcs "$archive" "$handler_object" "$lifecycle_object" \
  "$hierarchy_object" "$requested_layer_object" "$creation_args_object" \
  "$transaction_bridge_object" "$layer_state_factory_object"
binder_archive="$artifact_stage/libbinder-darwin.a"
gui_archive="$artifact_stage/libgui-transaction-darwin.a"
fence_archive="$artifact_stage/libui-fence-darwin.a"
"$ar" rcs "$binder_archive" "${binder_objects[@]}"
"$ar" rcs "$gui_archive" "${gui_runtime_objects[@]}"
"$ar" rcs "$fence_archive" "$fence_object" "$fence_sync_object" \
  "$fence_time_object"
definitions="$(nm -gUC "$archive")"
for symbol in \
  'android::surfaceflinger::frontend::TransactionHandler::queueTransaction(android::QueuedTransactionState&&)' \
  'android::surfaceflinger::frontend::TransactionHandler::flushTransactions()'; do
  grep -F " T $symbol" <<<"$definitions" >/dev/null ||
    fail_build "missing frontend definition $symbol"
done

nm -u "$archive" | awk '$1 ~ /^_/ { print $1 }' | sort -u \
  > "$artifact_stage/undefined-symbols.txt"
printf '%s\n' \
  "frameworks-native=$FRAMEWORKS_NATIVE_REVISION" \
  "hardware-interfaces=$HARDWARE_INTERFACES_REVISION" \
  "system-libhidl=$SYSTEM_LIBHIDL_REVISION" \
  "system-libfmq=$SYSTEM_LIBFMQ_REVISION" \
  > "$artifact_stage/source-identity.txt"

mv "$archive" "$output_root/libsurfaceflinger-frontend-darwin.a"
mv "$binder_archive" "$output_root/libbinder-darwin.a"
mv "$gui_archive" "$output_root/libgui-transaction-darwin.a"
mv "$fence_archive" "$output_root/libui-fence-darwin.a"
mv "$artifact_stage/undefined-symbols.txt" "$output_root/undefined-symbols.txt"
mv "$artifact_stage/source-identity.txt" "$output_root/source-identity.txt"

echo "surfaceflinger-core-build: archives=frontend,binder,gui,fence arch=arm64"
if [[ "$mode" == --archive-and-test ]]; then
  bash "$script_dir/test-android16-surfaceflinger-core.sh" --prepared
fi
