#!/bin/bash
set -euo pipefail
project_root="$(cd "$(dirname "$0")/.." && pwd)"
mode="${1:---rpc}"
case "$mode" in
  --rpc) bash "$project_root/tools/build-android16-surfaceflinger-core.sh" ;;
  --kernel-core) bash "$project_root/tools/build-original-binder-core.sh" --archive ;;
  *) echo "Usage: $0 [--rpc|--kernel-core]" >&2; exit 2 ;;
esac
sf="$project_root/_build/surfaceflinger-core"
shadow="$sf/work/frameworks-native"
generated="$sf/work/generated"
libhidl="$project_root/_aosp/android16-surfaceflinger-core/system-libhidl"
libfmq="$project_root/_aosp/android16-surfaceflinger-core/system-libfmq"
sdk_root="$(xcrun --sdk macosx --show-sdk-path)"
source "$project_root/tools/lib/surfaceflinger-compile-flags.sh"
out="$project_root/_build/binder-ndk-rpc-test"
binder_archive="$sf/libbinder-darwin.a"
if [[ "$mode" == --kernel-core ]]; then
  # Link original RPC ownership against the consistent kernel-capable archive.
  # This checks coexistence/link closure, NOT a /dev/binder transport.
  out="$project_root/_build/binder-ndk-rpc-kernel-test"
  binder_archive="$project_root/_build/original-binder-core/libbinder-kernel-darwin.a"
  flags+=(-DBINDER_WITH_KERNEL_IPC)
fi
mkdir -p "$out"
objects=()
for unit in ibinder libbinder parcel stability status; do
  xcrun clang++ "${flags[@]}" -UANDROID_UTILS_REF_BASE_DISABLE_IMPLICIT_CONSTRUCTION \
    -I"$shadow/libs/binder/ndk/include_platform" \
    -I"$project_root/_aosp/external/fmtlib/include" \
    -c "$shadow/libs/binder/ndk/$unit.cpp" -o "$out/$unit.o"
  objects+=("$out/$unit.o")
done
xcrun clang++ "${flags[@]}" -I"$shadow/libs/binder/ndk/include_platform" \
  "$project_root/tools/tests/binder-ndk-rpc-test.cc" "${objects[@]}" \
  "$binder_archive" \
  "$project_root/_build/libbase-foundation/libandroid-base-darwin.a" \
  "$project_root/_build/graphics-foundations/libutils-darwin.a" \
  "$project_root/_build/graphics-foundations/libcutils-darwin.a" \
  "$project_root/_build/graphics-foundations/liblog-darwin.a" \
  -Wl,-dead_strip -o "$out/ndk-rpc-test"
echo "Built diagnostic executable: $out/ndk-rpc-test (not yet run)"
