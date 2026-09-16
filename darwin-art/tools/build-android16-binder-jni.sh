#!/bin/bash
# Build the pinned AOSP Java Binder/Parcel JNI owners as an isolated archive.
# Production registration/cutover is deliberately a separate verified step.
set -euo pipefail
export LC_ALL=C

root="$(cd "$(dirname "$0")/.." && pwd)"
project_root="$root"
source "$root/upstream/android16-binder-jni.lock"
source_root="$root/_aosp/android16-binder-jni"
out="$root/_build/binder-jni"
shadow="$root/_build/surfaceflinger-core/work/frameworks-native"
generated="$root/_build/surfaceflinger-core/work/generated"
libhidl="$root/_aosp/android16-surfaceflinger-core/system-libhidl"
libfmq="$root/_aosp/android16-surfaceflinger-core/system-libfmq"
helpers="$root/_aosp/frameworks-base-android-util-log/core/jni"
nativehelper="$root/_build/nativehelper-foundation/source/libnativehelper"
sdk_root="$(xcrun --sdk macosx --show-sdk-path)"
uapi="$root/_aosp/android16-surfaceflinger-core/binder-uapi/libc/kernel"
mkdir -p "$source_root/core/jni" \
  "$source_root/binderthreadstate/binderthreadstate" "$out"

sha256() { shasum -a 256 "$1" | awk '{print $1}'; }
materialize() {
  local destination="$1" url="$2" expected="$3" staged
  if [[ ! -f "$destination" ]]; then
    staged="$(mktemp "$source_root/download.XXXXXX")"
    if ! curl -fsSL "$url?format=TEXT" | base64 -D > "$staged"; then
      rm -f "$staged"
      return 1
    fi
    [[ "$(sha256 "$staged")" == "$expected" ]] || {
      rm -f "$staged"
      echo "binder-jni: source hash mismatch: $destination" >&2
      return 1
    }
    mv "$staged" "$destination"
  fi
  [[ "$(sha256 "$destination")" == "$expected" ]] || {
    echo "binder-jni: source hash mismatch: $destination" >&2
    return 1
  }
}

materialize "$source_root/core/jni/android_os_Parcel.cpp" \
  "https://android.googlesource.com/$FRAMEWORKS_BASE_PROJECT/+/$FRAMEWORKS_BASE_REVISION/core/jni/android_os_Parcel.cpp" \
  "$ANDROID_OS_PARCEL_CPP_SHA256"
materialize "$source_root/core/jni/android_os_Parcel.h" \
  "https://android.googlesource.com/$FRAMEWORKS_BASE_PROJECT/+/$FRAMEWORKS_BASE_REVISION/core/jni/android_os_Parcel.h" \
  "$ANDROID_OS_PARCEL_H_SHA256"
materialize "$source_root/core/jni/android_util_Binder.cpp" \
  "https://android.googlesource.com/$FRAMEWORKS_BASE_PROJECT/+/$FRAMEWORKS_BASE_REVISION/core/jni/android_util_Binder.cpp" \
  "$ANDROID_UTIL_BINDER_CPP_SHA256"
materialize "$source_root/core/jni/android_util_Binder.h" \
  "https://android.googlesource.com/$FRAMEWORKS_BASE_PROJECT/+/$FRAMEWORKS_BASE_REVISION/core/jni/android_util_Binder.h" \
  "$ANDROID_UTIL_BINDER_H_SHA256"
materialize "$source_root/binderthreadstate/binderthreadstate/CallerUtils.h" \
  "https://android.googlesource.com/$FRAMEWORKS_NATIVE_PROJECT/+/$FRAMEWORKS_NATIVE_REVISION/libs/binderthreadstate/include/binderthreadstate/CallerUtils.h" \
  "$CALLER_UTILS_H_SHA256"

thread_patch="$root/patches/frameworks-base/0001-darwin-binder-rpc-thread-attach.patch"
identity_patch="$root/patches/frameworks-base/0002-darwin-binder-rpc-calling-identity.patch"
context_patch="$root/patches/frameworks-base/0003-darwin-binder-rpc-context-manager.patch"
thread_state_patch="$root/patches/frameworks-base/0004-darwin-binder-rpc-thread-state.patch"
parcel_state_patch="$root/patches/frameworks-base/0005-darwin-parcel-rpc-thread-state.patch"
critical_jni_patch="$root/patches/frameworks-base/0006-darwin-core-critical-jni-abi.patch"
fd_namespace_patch="$root/patches/frameworks-base/0007-darwin-parcel-fd-namespace.patch"
[[ "$(sha256 "$thread_patch")" == "$DARWIN_BINDER_RPC_THREAD_ATTACH_PATCH_SHA256" ]] || {
  echo "binder-jni: Darwin thread-attachment patch hash mismatch" >&2
  exit 1
}
[[ "$(sha256 "$identity_patch")" == "$DARWIN_BINDER_RPC_CALLING_IDENTITY_PATCH_SHA256" ]] || {
  echo "binder-jni: Darwin calling-identity patch hash mismatch" >&2
  exit 1
}
[[ "$(sha256 "$context_patch")" == "$DARWIN_BINDER_RPC_CONTEXT_MANAGER_PATCH_SHA256" ]] || {
  echo "binder-jni: Darwin context-manager patch hash mismatch" >&2
  exit 1
}
[[ "$(sha256 "$thread_state_patch")" == "$DARWIN_BINDER_RPC_THREAD_STATE_PATCH_SHA256" ]] || {
  echo "binder-jni: Darwin RPC thread-state patch hash mismatch" >&2
  exit 1
}
[[ "$(sha256 "$parcel_state_patch")" == "$DARWIN_BINDER_RPC_PARCEL_STATE_PATCH_SHA256" ]] || {
  echo "binder-jni: Darwin RPC Parcel thread-state patch hash mismatch" >&2
  exit 1
}
[[ "$(sha256 "$critical_jni_patch")" == "$DARWIN_CORE_CRITICAL_JNI_ABI_PATCH_SHA256" ]] || {
  echo "binder-jni: Darwin core CriticalNative ABI patch hash mismatch" >&2
  exit 1
}
[[ "$(sha256 "$fd_namespace_patch")" == "$DARWIN_PARCEL_FD_NAMESPACE_PATCH_SHA256" ]] || {
  echo "binder-jni: Darwin Parcel FD namespace patch hash mismatch" >&2
  exit 1
}
patched_root="$out/patched-source"
patch_fingerprint="$({
  shasum -a 256 "$source_root/core/jni/android_os_Parcel.cpp" \
    "$source_root/core/jni/android_os_Parcel.h" \
    "$source_root/core/jni/android_util_Binder.cpp" \
    "$source_root/core/jni/android_util_Binder.h" \
    "$helpers/core_jni_helpers.h" "$thread_patch" \
    "$identity_patch" "$context_patch" "$thread_state_patch" \
    "$parcel_state_patch" "$critical_jni_patch" "$fd_namespace_patch"
} | shasum -a 256 | awk '{print $1}')"
if [[ ! -f "$patched_root/.fingerprint" || \
      "$(<"$patched_root/.fingerprint")" != "$patch_fingerprint" ]]; then
  patched_stage="$(mktemp -d "$out/patched-source.XXXXXX")"
  mkdir -p "$patched_stage/core/jni"
  cp "$source_root/core/jni"/android_os_Parcel.{cpp,h} \
    "$source_root/core/jni"/android_util_Binder.{cpp,h} \
    "$patched_stage/core/jni/"
  cp "$helpers/core_jni_helpers.h" "$patched_stage/core/jni/"
  patch --batch --fuzz=0 -d "$patched_stage" -p1 < "$thread_patch"
  patch --batch --fuzz=0 -d "$patched_stage" -p1 < "$identity_patch"
  patch --batch --fuzz=0 -d "$patched_stage" -p1 < "$context_patch"
  patch --batch --fuzz=0 -d "$patched_stage" -p1 < "$thread_state_patch"
  patch --batch --fuzz=0 -d "$patched_stage" -p1 < "$parcel_state_patch"
  patch --batch --fuzz=0 -d "$patched_stage" -p1 < "$critical_jni_patch"
  patch --batch --fuzz=0 -d "$patched_stage" -p1 < "$fd_namespace_patch"
  printf '%s\n' "$patch_fingerprint" > "$patched_stage/.fingerprint"
  rm -rf -- "$patched_root"
  mv "$patched_stage" "$patched_root"
fi

[[ -f "$shadow/libs/binder/include/binder/Parcel.h" ]] || {
  echo "binder-jni: build SurfaceFlinger core first" >&2
  exit 1
}
[[ -f "$helpers/core_jni_helpers.h" && -f "$nativehelper/include_jni/jni.h" ]] || {
  echo "binder-jni: framework JNI/nativehelper foundations missing" >&2
  exit 1
}

source "$root/tools/lib/surfaceflinger-compile-flags.sh"
flags+=(
  -D__ANDROID_API__=36 '-D__INTRODUCED_IN(n)='
  '-D__BIONIC_AVAILABILITY_GUARD(n)=1'
  -I"$patched_root/core/jni" -I"$source_root/binderthreadstate"
  -I"$root/compat/binder"
  -I"$helpers" -I"$helpers/include"
  -I"$nativehelper/include_jni" -I"$nativehelper/include"
  -I"$nativehelper/include_platform" -I"$nativehelper/include_platform_header_only"
  -I"$nativehelper/header_only_include"
  -I"$root/_aosp/android16-surfaceflinger-core/system-libhwbinder/include"
  -I"$uapi/uapi" -I"$uapi/uapi/asm-arm64" -I"$uapi/android/uapi"
  -I"$root/_aosp/external/fmtlib/include"
  -DBINDER_WITH_KERNEL_IPC -DBUILDING_LIBBINDER
  -DBINDER_ENABLE_LIBLOG_ASSERT -DBINDER_DISABLE_BLOB
  -DDARWIN_ART_ANDROID_CRITICAL_JNI_ABI
  -include android-base/macros.h
  -include "$root/compat/binder/device_uapi.h"
  -include "$root/compat/surfaceflinger/binder_socket_darwin.h"
)

objects=()
for unit in android_os_Parcel android_util_Binder; do
  object="$out/$unit.o"
  xcrun clang++ "${flags[@]}" -c "$patched_root/core/jni/$unit.cpp" -o "$object"
  objects+=("$object")
done
xcrun libtool -static -o "$out/libbinder-jni-darwin.a" "${objects[@]}"

boundary_objects=()
rpc_context_object="$out/rpc-context.o"
xcrun clang++ "${flags[@]}" -I"$root/compat" \
  -c "$root/compat/binder/rpc_context.cc" -o "$rpc_context_object"
boundary_objects+=("$rpc_context_object")
rpc_context_server_object="$out/rpc-context-server.o"
xcrun clang++ "${flags[@]}" -I"$root/compat" \
  -c "$root/compat/binder/rpc_context_server.cc" \
  -o "$rpc_context_server_object"
boundary_objects+=("$rpc_context_server_object")
rpc_identity_object="$out/rpc-identity.o"
xcrun clang++ "${flags[@]}" -I"$root/compat" \
  -c "$root/compat/binder/rpc_identity.cc" -o "$rpc_identity_object"
boundary_objects+=("$rpc_identity_object")
fd_transport_object="$out/fd-transport.o"
xcrun clang++ "${flags[@]}" -I"$root/compat" \
  -c "$root/compat/binder/fd_transport.cc" -o "$fd_transport_object"
boundary_objects+=("$fd_transport_object")
xcrun libtool -static -o "$out/libbinder-rpc-boundary-darwin.a" \
  "${boundary_objects[@]}"
nm -gU "$out/libbinder-jni-darwin.a" | c++filt > "$out/exports.txt"
rg -Fq 'register_android_os_Parcel' "$out/exports.txt"
rg -Fq 'register_android_os_Binder' "$out/exports.txt"
nm -gU "$out/libbinder-rpc-boundary-darwin.a" | c++filt \
  > "$out/boundary-exports.txt"
rg -Fq 'darwin_art::binder::ConnectRpcContext' "$out/boundary-exports.txt"
rg -Fq 'darwin_art::binder::ServeRpcContext' "$out/boundary-exports.txt"
shasum -a 256 "$source_root/core/jni/android_os_Parcel.cpp" \
  "$source_root/core/jni/android_os_Parcel.h" \
  "$source_root/core/jni/android_util_Binder.cpp" \
  "$source_root/core/jni/android_util_Binder.h" \
  "$helpers/core_jni_helpers.h" \
  "$source_root/binderthreadstate/binderthreadstate/CallerUtils.h" \
  "$thread_patch" \
  "$identity_patch" \
  "$context_patch" \
  "$thread_state_patch" \
  "$root/compat/binder/rpc_context.h" \
  "$root/compat/binder/rpc_context.cc" \
  "$root/compat/binder/rpc_context_server.h" \
  "$root/compat/binder/rpc_context_server.cc" \
  "$root/compat/binder/rpc_identity.h" \
  "$root/compat/binder/rpc_identity.cc" \
  "$parcel_state_patch" \
  "$critical_jni_patch" \
  "$fd_namespace_patch" \
  "$root/compat/binder/fd_transport.h" \
  "$root/compat/binder/fd_transport.cc" \
  "$root/upstream/android16-binder-jni.lock" \
  "$root/tools/build-android16-binder-jni.sh" > "$out/source-identity.txt"
echo "binder-jni: pinned AOSP Binder/Parcel JNI archive compiled; production registration NOT changed"
