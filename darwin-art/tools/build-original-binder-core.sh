#!/bin/bash
# Compile original Binder kernel clients, optionally a consistently configured
# archive, without installing into production. Device transport is still missing.
set -euo pipefail
mode="${1:---objects}"
case "$mode" in
  --objects|--archive) ;;
  *) echo "Usage: $0 [--objects|--archive]" >&2; exit 2 ;;
esac
project_root="$(cd "$(dirname "$0")/.." && pwd)"
source "$project_root/upstream/android16-binder-jni.lock"
shadow="$project_root/_build/surfaceflinger-core/work/frameworks-native"
generated="$project_root/_build/surfaceflinger-core/work/generated"
libhidl="$project_root/_aosp/android16-surfaceflinger-core/system-libhidl"
libfmq="$project_root/_aosp/android16-surfaceflinger-core/system-libfmq"
sdk_root="$(xcrun --sdk macosx --show-sdk-path)"
source "$project_root/tools/lib/surfaceflinger-compile-flags.sh"
uapi="$project_root/_aosp/android16-surfaceflinger-core/binder-uapi/libc/kernel"
out="$project_root/_build/original-binder-core"
mkdir -p "$out"

sha256() { shasum -a 256 "$1" | awk '{print $1}'; }
rpc_identity_patch="$project_root/patches/frameworks-native/0006-darwin-binder-rpc-peer-identity.patch"
[[ "$(sha256 "$rpc_identity_patch")" == "$DARWIN_BINDER_RPC_PEER_IDENTITY_PATCH_SHA256" ]] || {
  echo "original Binder: Darwin RPC peer-identity patch hash mismatch" >&2
  exit 1
}
patched_root="$out/patched-source"
patch_fingerprint="$({
  shasum -a 256 "$shadow/libs/binder/RpcServer.cpp" \
    "$shadow/libs/binder/RpcSession.cpp" "$shadow/libs/binder/RpcState.cpp" \
    "$rpc_identity_patch" "$project_root/tools/build-original-binder-core.sh"
} | shasum -a 256 | awk '{print $1}')"
if [[ ! -f "$patched_root/.fingerprint" || \
      "$(<"$patched_root/.fingerprint")" != "$patch_fingerprint" ]]; then
  patched_stage="$(mktemp -d "$out/patched-source.XXXXXX")"
  mkdir -p "$patched_stage/libs/binder"
  cp "$shadow/libs/binder/RpcServer.cpp" \
    "$shadow/libs/binder/RpcSession.cpp" "$shadow/libs/binder/RpcState.cpp" \
    "$patched_stage/libs/binder/"
  # The canonical SurfaceFlinger shadow may already contain this patch. Never
  # let `patch --batch` guess that it should reverse an already-applied hunk:
  # that silently produced a Binder archive without authenticated RPC hooks.
  if ! rg -q 'darwin_art_binder_rpc_identity_bind' \
      "$patched_stage/libs/binder/RpcServer.cpp" || \
     ! rg -q 'darwin_art_binder_rpc_identity_forget' \
      "$patched_stage/libs/binder/RpcSession.cpp" || \
     ! rg -q 'darwin_art_binder_rpc_identity_enter' \
      "$patched_stage/libs/binder/RpcState.cpp"; then
    patch --batch --forward --fuzz=0 -d "$patched_stage" -p1 \
      < "$rpc_identity_patch"
  fi
  printf '%s\n' "$patch_fingerprint" > "$patched_stage/.fingerprint"
  rm -rf -- "$patched_root"
  mv "$patched_stage" "$patched_root"
fi

compile_binder_object() {
  local source="$1" object="$2" stamp="$2.fingerprint" fingerprint staged
  fingerprint="$({
    shasum -a 256 "$source" "$shadow/libs/binder/binder_module.h" \
      "$project_root/compat/binder/device_uapi.h" \
      "$project_root/compat/surfaceflinger/binder_socket_darwin.h" \
      "$project_root/tools/lib/surfaceflinger-compile-flags.sh"
    printf '%s\n' "${flags[@]}"
  } | shasum -a 256 | awk '{print $1}')"
  if [[ -f "$object" && -f "$stamp" && "$(<"$stamp")" == "$fingerprint" ]]; then
    return
  fi
  staged="$object.tmp.$$"
  xcrun clang++ "${flags[@]}" -UANDROID_UTILS_REF_BASE_DISABLE_IMPLICIT_CONSTRUCTION \
    -DBUILDING_LIBBINDER -DBINDER_ENABLE_LIBLOG_ASSERT -DBINDER_DISABLE_BLOB \
    -DBINDER_WITH_KERNEL_IPC -DLOG_NDEBUG=1 \
    -I"$uapi/uapi" -I"$uapi/uapi/asm-arm64" -I"$uapi/android/uapi" \
    -I"$project_root/_aosp/external/fmtlib/include" \
    -I"$project_root/compat/binder" \
    -I"$shadow/libs/binder" \
    -include android-base/macros.h \
    -include "$project_root/compat/binder/device_uapi.h" \
    -include "$project_root/compat/surfaceflinger/binder_socket_darwin.h" \
    -c "$source" -o "$staged"
  mv "$staged" "$object"
  printf '%s\n' "$fingerprint" > "$stamp"
}
xcrun clang++ -std=c++23 -Wall -Wextra -Werror \
  -I"$project_root/compat" -I"$uapi/uapi" -I"$uapi/uapi/asm-arm64" \
  -I"$uapi/android/uapi" "$project_root/tools/tests/binder-device-uapi-test.cc" \
  -o "$out/device-uapi-test"
"$out/device-uapi-test"
sources=()
units=(IPCThreadState ProcessState Static BufferedTextOutput)
if [[ "$mode" == --archive ]]; then
  # Never mix these with the production RPC-only archive: Binder/Parcel and
  # their kernel clients must agree on binder_module.h feature selection.
  units+=(Binder BpBinder Debug FdTrigger IInterface IResultReceiver Parcel
    ParcelFileDescriptor RpcSession RpcServer RpcState RpcTransportRaw Stability
    Status TextOutput Utils file OS_unix_base RecordedTransaction)
fi
objects=()
for unit in "${units[@]}"; do
  source_file="$shadow/libs/binder/$unit.cpp"
  case "$unit" in
    RpcServer|RpcSession|RpcState)
      source_file="$patched_root/libs/binder/$unit.cpp"
      ;;
  esac
  sources+=("$source_file")
  compile_binder_object "$source_file" "$out/$unit.o"
  objects+=("$out/$unit.o")
done
if [[ "$mode" == --archive ]]; then
  sources+=("$project_root/compat/surfaceflinger/binder_os_darwin.cc")
  xcrun clang++ "${flags[@]}" -DBUILDING_LIBBINDER \
    -DBINDER_WITH_KERNEL_IPC -DBINDER_ENABLE_LIBLOG_ASSERT -DBINDER_DISABLE_BLOB \
    -I"$shadow/libs/binder" \
    -include "$project_root/compat/surfaceflinger/binder_socket_darwin.h" \
    -c "${sources[${#sources[@]}-1]}" -o "$out/os-darwin.o"
  objects+=("$out/os-darwin.o")
  xcrun libtool -static -o "$out/libbinder-kernel-darwin.a" "${objects[@]}"
fi
nm -gU "$out/IPCThreadState.o" | c++filt > "$out/thread-exports.txt"
rg -Fq 'android::IPCThreadState::flushCommands()' "$out/thread-exports.txt"
rg -Fq 'android::IPCThreadState::self()' "$out/thread-exports.txt"
nm -u "$out/IPCThreadState.o" "$out/ProcessState.o" > "$out/platform-imports.txt"
for symbol in ioctl open mmap munmap pthread_atfork; do
  rg -q "(^|[[:space:]])_${symbol}$" "$out/platform-imports.txt"
done
shasum -a 256 "${sources[@]}" "$shadow/libs/binder/binder_module.h" \
  "$uapi/uapi/linux/android/binder.h" \
  "$project_root/compat/binder/device_uapi.h" \
  "$project_root/compat/surfaceflinger/binder_socket_darwin.h" \
  "$rpc_identity_patch" \
  "$project_root/tools/lib/surfaceflinger-compile-flags.sh" \
  "$project_root/tools/build-original-binder-core.sh" > "$out/source-identity.txt"
echo "Original Binder $mode compiled; transport/link/runtime NOT verified"
