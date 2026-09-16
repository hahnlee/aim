#!/bin/bash
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
bash "$root/tools/build-original-binder-core.sh" --archive
shadow="$root/_build/surfaceflinger-core/work/frameworks-native"
generated="$root/_build/surfaceflinger-core/work/generated"
libhidl="$root/_aosp/android16-surfaceflinger-core/system-libhidl"
libfmq="$root/_aosp/android16-surfaceflinger-core/system-libfmq"
sdk_root="$(xcrun --sdk macosx --show-sdk-path)"
project_root="$root"
source "$root/tools/lib/surfaceflinger-compile-flags.sh"
out="$root/_build/binder-rpc-context-test"
mkdir -p "$out"
xcrun clang++ "${flags[@]}" -DBINDER_WITH_KERNEL_IPC -I"$root/compat" \
  "$root/tools/tests/binder-rpc-context-test.cc" \
  "$root/compat/binder/rpc_context.cc" \
  "$root/compat/binder/rpc_identity.cc" \
  "$root/compat/binder/calling_identity.cc" \
  "$root/compat/binder/peer_credentials.cc" \
  "$root/_build/original-binder-core/libbinder-kernel-darwin.a" \
  "$root/_build/libbase-foundation/libandroid-base-darwin.a" \
  "$root/_build/graphics-foundations/libutils-darwin.a" \
  "$root/_build/graphics-foundations/libcutils-darwin.a" \
  "$root/_build/graphics-foundations/liblog-darwin.a" \
  -Wl,-dead_strip -o "$out/binder-rpc-context-test"
"$out/binder-rpc-context-test"
