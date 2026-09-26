#!/bin/bash
set -euo pipefail
project_root="$(cd "$(dirname "$0")/../.." && pwd)"
bash "$project_root/tools/materialize-android16-key-character-map.sh"
native_root="$project_root/_aosp/android16-key-character-map/frameworks/native"
task_stage="$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-kcm-runtime.XXXXXX")"
trap 'rm -rf -- "$task_stage"' EXIT
source "$project_root/tools/lib/key-character-map-compile-context.sh"
objects=()
for unit in KeyCharacterMap InputEventLabels Keyboard Input InputDevice KeyLayoutMap PropertyMap; do
  object="$task_stage/$unit.o"
  xcrun clang++ "${kcm_flags[@]}" -c "$native_root/libs/input/$unit.cpp" -o "$object"
  objects+=("$object")
done
# Use real production Binder transport/identity and Rust provider ownership.
# No test implementations of Parcel, map matching, registered UID or syscalls.
for unit in rpc_identity calling_identity peer_credentials fd_transport platform_syscalls; do
  object="$task_stage/binder-$unit.o"
  xcrun clang++ "${kcm_flags[@]}" -I"$project_root/compat" \
    -I"$project_root/tools/bionic-fs-facade/include" \
    -I"$project_root/tools/bionic-central-fd-broker/include" \
    -I"$project_root/tools/bionic-socket-broker-adapter/src" \
    -I"$project_root/tools/bionic-ioctl-facade/include" \
    -I"$project_root/tools/bionic-vm-facade/include" \
    -c "$project_root/compat/binder/$unit.cc" -o "$object"
  objects+=("$object")
done
xcrun clang++ "${kcm_flags[@]}" -c \
  "$project_root/tools/tests/aosp-key-character-map-test.cc" -o "$task_stage/test.o"
xcrun clang++ -arch arm64 -isysroot "$sdk_root" -Wl,-dead_strip \
  "$task_stage/test.o" "${objects[@]}" \
  "$project_root/_build/surfaceflinger-core/libbinder-darwin.a" \
  "$project_root/_build/ui-types-foundation/libui-types.a" \
  "$project_root/_build/graphics-foundations/libutils-darwin.a" \
  "$project_root/_build/graphics-foundations/libcutils-darwin.a" \
  "$project_root/_build/graphics-foundations/liblog-darwin.a" \
  "$project_root/_build/libbase-foundation/libandroid-base-darwin.a" \
  "$project_root/target/debug/libdarwin_art_runtime.a" \
  "$project_root/_build/bionic-runtime-provider-closure/libdarwin-art-bionic-native-providers.a" \
  "$project_root/_build/bionic-runtime-provider-closure/libdarwin-art-bionic-rust-providers.a" \
  "$project_root/_build/bionic-runtime-provider-closure/libdarwin-art-bionic-float-conversion.a" \
  -framework Foundation -framework AppKit -framework Security -framework IOKit \
  -lresolv \
  -o "$task_stage/test"
"$task_stage/test" \
  "$project_root/_build/android16-key-character-map/system/usr/keychars/Generic.kcm" \
  "$project_root/_build/android16-key-character-map/system/usr/keychars/Virtual.kcm"
