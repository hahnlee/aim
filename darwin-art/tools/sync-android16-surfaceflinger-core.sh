#!/bin/bash
set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/.." && pwd)"
lock_file="$project_root/upstream/android16-surfaceflinger-core.lock"
download_root="$project_root/_downloads/android16-surfaceflinger-core"
source_root="$project_root/_aosp/android16-surfaceflinger-core"
tool_root="$download_root/tools"

# shellcheck disable=SC1090
source "$lock_file"

fail_sync() {
  echo "surfaceflinger-core-sync: $1" >&2
  exit 2
}

ensure_repo() {
  local name="$1" project="$2" revision="$3"
  local repo="$download_root/repos/$name"
  if [[ ! -d "$repo/.git" ]]; then
    mkdir -p "$repo"
    git -C "$repo" init -q
    git -C "$repo" remote add origin "https://android.googlesource.com/$project"
  fi
  if ! git -C "$repo" cat-file -e "$revision^{commit}" 2>/dev/null; then
    git -C "$repo" fetch -q --depth=1 origin "$revision"
  fi
  printf '%s\n' "$repo"
}

verify_tree() {
  local repo="$1" revision="$2" path="$3" expected="$4"
  local actual
  actual="$(git -C "$repo" rev-parse "$revision:$path")"
  [[ "$actual" == "$expected" ]] ||
    fail_sync "tree mismatch path=$path expected=$expected actual=$actual"
}

export_paths() {
  local repo="$1" revision="$2" destination="$3"
  shift 3
  local stage
  stage="$(mktemp -d "$source_root/.stage.XXXXXX")"
  git -C "$repo" archive "$revision" "$@" | tar -x -C "$stage"
  mkdir -p "$(dirname "$destination")"
  if [[ -e "$destination" ]]; then
    local old
    old="${destination}.old.$$"
    mv "$destination" "$old"
    mv "$stage" "$destination"
    rm -rf "$old"
  else
    mv "$stage" "$destination"
  fi
}

mkdir -p "$download_root/repos" "$source_root" "$tool_root"

system_core_repo="$(ensure_repo system-core "$SYSTEM_CORE_PROJECT" "$SYSTEM_CORE_REVISION")"
verify_tree "$system_core_repo" "$SYSTEM_CORE_REVISION" libvndksupport "$SYSTEM_CORE_LIBVNDKSUPPORT_TREE"
if [[ ! -f "$source_root/vndksupport-identity" || \
      "$(cat "$source_root/vndksupport-identity")" != "$SYSTEM_CORE_LIBVNDKSUPPORT_TREE" ]]; then
  export_paths "$system_core_repo" "$SYSTEM_CORE_REVISION" "$source_root/system-core-vndksupport" libvndksupport
  printf '%s\n' "$SYSTEM_CORE_LIBVNDKSUPPORT_TREE" > "$source_root/vndksupport-identity"
fi

frameworks_native_repo="$(ensure_repo frameworks-native "$FRAMEWORKS_NATIVE_PROJECT" "$FRAMEWORKS_NATIVE_REVISION")"
verify_tree "$frameworks_native_repo" "$FRAMEWORKS_NATIVE_REVISION" libs/ui "$FRAMEWORKS_NATIVE_LIBS_UI_TREE"
verify_tree "$frameworks_native_repo" "$FRAMEWORKS_NATIVE_REVISION" libs/gralloc/types "$FRAMEWORKS_NATIVE_LIBS_GRALLOC_TYPES_TREE"
verify_tree "$frameworks_native_repo" "$FRAMEWORKS_NATIVE_REVISION" libs/nativewindow "$FRAMEWORKS_NATIVE_LIBS_NATIVEWINDOW_TREE"
verify_tree "$system_core_repo" "$SYSTEM_CORE_REVISION" libgrallocusage "$SYSTEM_CORE_LIBGRALLOCUSAGE_TREE"
verify_tree "$system_core_repo" "$SYSTEM_CORE_REVISION" libsync "$SYSTEM_CORE_LIBSYNC_TREE"
buffer_identity="$FRAMEWORKS_NATIVE_LIBS_UI_TREE:$FRAMEWORKS_NATIVE_LIBS_NATIVEWINDOW_TREE:$SYSTEM_CORE_LIBGRALLOCUSAGE_TREE:$SYSTEM_CORE_LIBSYNC_TREE:$FRAMEWORKS_NATIVE_LIBS_GRALLOC_TYPES_TREE"
if [[ ! -f "$source_root/buffer-identity" || "$(cat "$source_root/buffer-identity")" != "$buffer_identity" ]]; then
  export_paths "$frameworks_native_repo" "$FRAMEWORKS_NATIVE_REVISION" "$source_root/buffer-native" libs/ui libs/nativewindow libs/gralloc/types
  export_paths "$system_core_repo" "$SYSTEM_CORE_REVISION" "$source_root/buffer-core" libgrallocusage libsync
  printf '%s\n' "$buffer_identity" > "$source_root/buffer-identity"
fi
verify_tree "$frameworks_native_repo" "$FRAMEWORKS_NATIVE_REVISION" services/surfaceflinger "$FRAMEWORKS_NATIVE_SERVICES_SURFACEFLINGER_TREE"
verify_tree "$frameworks_native_repo" "$FRAMEWORKS_NATIVE_REVISION" libs/renderengine "$FRAMEWORKS_NATIVE_LIBS_RENDERENGINE_TREE"
verify_tree "$frameworks_native_repo" "$FRAMEWORKS_NATIVE_REVISION" libs/ftl "$FRAMEWORKS_NATIVE_LIBS_FTL_TREE"
verify_tree "$frameworks_native_repo" "$FRAMEWORKS_NATIVE_REVISION" libs/binder "$FRAMEWORKS_NATIVE_LIBS_BINDER_TREE"
verify_tree "$frameworks_native_repo" "$FRAMEWORKS_NATIVE_REVISION" libs/gui "$FRAMEWORKS_NATIVE_LIBS_GUI_TREE"
verify_tree "$frameworks_native_repo" "$FRAMEWORKS_NATIVE_REVISION" libs/input "$FRAMEWORKS_NATIVE_LIBS_INPUT_TREE"
verify_tree "$frameworks_native_repo" "$FRAMEWORKS_NATIVE_REVISION" aidl/gui "$FRAMEWORKS_NATIVE_AIDL_GUI_TREE"

hardware_interfaces_repo="$(ensure_repo hardware-interfaces "$HARDWARE_INTERFACES_PROJECT" "$HARDWARE_INTERFACES_REVISION")"
verify_tree "$hardware_interfaces_repo" "$HARDWARE_INTERFACES_REVISION" common/aidl "$HARDWARE_INTERFACES_COMMON_AIDL_TREE"
verify_tree "$hardware_interfaces_repo" "$HARDWARE_INTERFACES_REVISION" graphics/common "$HARDWARE_INTERFACES_GRAPHICS_COMMON_TREE"
verify_tree "$hardware_interfaces_repo" "$HARDWARE_INTERFACES_REVISION" graphics/bufferqueue "$HARDWARE_INTERFACES_GRAPHICS_BUFFERQUEUE_TREE"
verify_tree "$hardware_interfaces_repo" "$HARDWARE_INTERFACES_REVISION" graphics/allocator "$HARDWARE_INTERFACES_GRAPHICS_ALLOCATOR_TREE"
verify_tree "$hardware_interfaces_repo" "$HARDWARE_INTERFACES_REVISION" common/support "$HARDWARE_INTERFACES_COMMON_SUPPORT_TREE"
verify_tree "$hardware_interfaces_repo" "$HARDWARE_INTERFACES_REVISION" graphics/mapper "$HARDWARE_INTERFACES_GRAPHICS_MAPPER_TREE"
verify_tree "$hardware_interfaces_repo" "$HARDWARE_INTERFACES_REVISION" graphics/composer/aidl "$HARDWARE_INTERFACES_GRAPHICS_COMPOSER_AIDL_TREE"
verify_tree "$hardware_interfaces_repo" "$HARDWARE_INTERFACES_REVISION" media/1.0 "$HARDWARE_INTERFACES_MEDIA_1_0_TREE"

libhidl_repo="$(ensure_repo system-libhidl "$SYSTEM_LIBHIDL_PROJECT" "$SYSTEM_LIBHIDL_REVISION")"
[[ "$(git -C "$libhidl_repo" rev-parse "$SYSTEM_LIBHIDL_REVISION^{tree}")" == "$SYSTEM_LIBHIDL_TREE" ]] ||
  fail_sync "system/libhidl tree mismatch"
libfmq_repo="$(ensure_repo system-libfmq "$SYSTEM_LIBFMQ_PROJECT" "$SYSTEM_LIBFMQ_REVISION")"
[[ "$(git -C "$libfmq_repo" rev-parse "$SYSTEM_LIBFMQ_REVISION^{tree}")" == "$SYSTEM_LIBFMQ_TREE" ]] ||
  fail_sync "system/libfmq tree mismatch"

identity="$FRAMEWORKS_NATIVE_REVISION:$HARDWARE_INTERFACES_REVISION:$SYSTEM_LIBHIDL_REVISION:$SYSTEM_LIBFMQ_REVISION:allocator-mapper-2"
bionic_repo="$(ensure_repo bionic-uapi "$BIONIC_UAPI_PROJECT" "$BIONIC_UAPI_REVISION")"
verify_tree "$bionic_repo" "$BIONIC_UAPI_REVISION" libc/kernel/uapi "$BIONIC_UAPI_TREE"
if [[ ! -f "$source_root/binder-uapi-identity" || \
      "$(cat "$source_root/binder-uapi-identity")" != "$BIONIC_UAPI_REVISION:4" ]]; then
  export_paths "$bionic_repo" "$BIONIC_UAPI_REVISION" "$source_root/binder-uapi" \
    libc/kernel/uapi/linux/android/binder.h libc/kernel/uapi/linux/types.h \
    libc/kernel/uapi/linux/posix_types.h libc/kernel/uapi/linux/stddef.h \
    libc/kernel/uapi/linux/ioctl.h libc/kernel/uapi/asm-arm64/asm/types.h \
    libc/kernel/uapi/linux/sched.h libc/kernel/uapi/linux/sync_file.h \
    libc/kernel/uapi/asm-arm64/asm/posix_types.h libc/kernel/uapi/asm-arm64/asm/ioctl.h \
    libc/kernel/uapi/asm-arm64/asm/bitsperlong.h libc/kernel/uapi/asm-generic/types.h \
    libc/kernel/uapi/asm-generic/int-ll64.h libc/kernel/uapi/asm-generic/posix_types.h \
    libc/kernel/uapi/asm-generic/ioctl.h libc/kernel/uapi/asm-generic/bitsperlong.h \
    libc/kernel/android/uapi/linux/compiler_types.h libc/kernel/android/uapi/linux/compiler.h
  printf '%s\n' "$BIONIC_UAPI_REVISION:4" > "$source_root/binder-uapi-identity"
fi
libhwbinder_repo="$(ensure_repo system-libhwbinder "$SYSTEM_LIBHWBINDER_PROJECT" "$SYSTEM_LIBHWBINDER_REVISION")"
[[ "$(git -C "$libhwbinder_repo" rev-parse "$SYSTEM_LIBHWBINDER_REVISION^{tree}")" == "$SYSTEM_LIBHWBINDER_TREE" ]] || \
  fail_sync "system/libhwbinder tree mismatch"
if [[ ! -f "$source_root/libhwbinder-identity" || \
      "$(cat "$source_root/libhwbinder-identity")" != "$SYSTEM_LIBHWBINDER_REVISION" || \
      ! -f "$source_root/system-libhwbinder/include/hwbinder/IBinder.h" ]]; then
  export_paths "$libhwbinder_repo" "$SYSTEM_LIBHWBINDER_REVISION" "$source_root/system-libhwbinder" .
  printf '%s\n' "$SYSTEM_LIBHWBINDER_REVISION" > "$source_root/libhwbinder-identity"
fi
if [[ ! -f "$source_root/identity" || "$(cat "$source_root/identity")" != "$identity" ]]; then
  export_paths "$frameworks_native_repo" "$FRAMEWORKS_NATIVE_REVISION" "$source_root/frameworks-native" \
    services/surfaceflinger libs/renderengine libs/ftl libs/binder libs/gui libs/input aidl/gui
  export_paths "$hardware_interfaces_repo" "$HARDWARE_INTERFACES_REVISION" "$source_root/hardware-interfaces" \
    common/aidl graphics/common graphics/bufferqueue graphics/composer/aidl media/1.0 \
    graphics/allocator graphics/mapper common/support
  export_paths "$libhidl_repo" "$SYSTEM_LIBHIDL_REVISION" "$source_root/system-libhidl" .
  export_paths "$libfmq_repo" "$SYSTEM_LIBFMQ_REVISION" "$source_root/system-libfmq" .
  printf '%s\n' "$identity" > "$source_root/identity"
fi

download_gitiles_tool() {
  local path="$1" destination="$2" expected="$3"
  if [[ ! -f "$destination" ]]; then
    local temporary
    temporary="$(mktemp "$destination.partial.XXXXXX")"
    curl -fsSL --retry 3 \
      "https://android.googlesource.com/$PREBUILTS_BUILD_TOOLS_PROJECT/+/$PREBUILTS_BUILD_TOOLS_REVISION/$path?format=TEXT" | \
      base64 -D > "$temporary"
    chmod +x "$temporary"
    mv "$temporary" "$destination"
  fi
  [[ "$(shasum -a 256 "$destination" | awk '{print $1}')" == "$expected" ]] ||
    fail_sync "tool checksum mismatch path=$destination"
}

download_gitiles_tool darwin-x86/bin/hidl-gen "$tool_root/hidl-gen" "$HIDL_GEN_SHA256"
if [[ "$(uname -m)" == arm64 ]] && ! arch -x86_64 /usr/bin/true 2>/dev/null; then
  # prebuilts/build-tools has only a darwin-x86 aconfig; without Rosetta it
  # cannot execute, so build the same tool from the pinned platform/build source.
  if [[ ! -x "$tool_root/aconfig" || "$(file -b "$tool_root/aconfig")" != *arm64* ]]; then
    DARWIN_ART_SURFACEFLINGER_CORE_TOOLS="$tool_root" \
      "$project_root/tools/build-android16-aconfig-host.sh"
  fi
else
  download_gitiles_tool darwin-x86/bin/aconfig "$tool_root/aconfig" "$ACONFIG_SHA256"
fi

aidl_zip="$download_root/build-tools-r36-macosx.zip"
if [[ ! -f "$aidl_zip" ]]; then
  curl -fL --retry 3 "$SDK_BUILD_TOOLS_URL" -o "$aidl_zip"
fi
[[ "$(shasum "$aidl_zip" | awk '{print $1}')" == "$SDK_BUILD_TOOLS_ZIP_SHA1" ]] ||
  fail_sync "SDK build-tools archive checksum mismatch"
if [[ ! -x "$tool_root/aidl" ]]; then
  aidl_stage="$(mktemp -d "$download_root/aidl.XXXXXX")"
  unzip -q "$aidl_zip" -d "$aidl_stage"
  aidl_binary="$(find "$aidl_stage" -type f -name aidl | head -1)"
  [[ -n "$aidl_binary" ]] || fail_sync "aidl binary missing from SDK archive"
  ditto "$aidl_binary" "$tool_root/aidl"
  chmod +x "$tool_root/aidl"
  rm -rf "$aidl_stage"
fi
[[ "$(shasum -a 256 "$tool_root/aidl" | awk '{print $1}')" == "$AIDL_SHA256" ]] ||
  fail_sync "aidl checksum mismatch"

echo "surfaceflinger-core-sync: source=$source_root"
echo "surfaceflinger-core-sync: tools=$tool_root"
