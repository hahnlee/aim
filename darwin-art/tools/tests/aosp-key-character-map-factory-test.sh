#!/bin/bash
set -euo pipefail

project_root="$(cd "$(dirname "$0")/../.." && pwd)"
bash "$project_root/tools/materialize-android16-key-character-map.sh"
native_root="$project_root/_aosp/android16-key-character-map/frameworks/native"
task_stage="$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-kcm-factory.XXXXXX")"
trap 'rm -rf -- "$task_stage"' EXIT

source "$project_root/tools/lib/key-character-map-compile-context.sh"
source "$project_root/tools/lib/key-character-map-jni-compile-context.sh"
objects=()
for unit in KeyCharacterMap InputEventLabels Keyboard Input InputDevice KeyLayoutMap PropertyMap; do
  object="$task_stage/$unit.o"
  xcrun clang++ "${kcm_flags[@]}" -ffunction-sections -fdata-sections \
    -c "$native_root/libs/input/$unit.cpp" -o "$object"
  objects+=("$object")
done

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

xcrun clang++ "${kcm_jni_flags[@]}" -ffunction-sections -fdata-sections \
  -DDARWIN_ART_KCM_JNI_SOURCE=\"$kcm_jni_root/android_view_KeyCharacterMap.cpp\" \
  -c "$project_root/tools/tests/aosp-key-character-map-factory-test.cc" \
  -o "$task_stage/factory-test.o"

# Link only the real production KCM, Binder, Rust-provider and foundation
# archives. No dynamic_lookup, unresolved-symbol suppression, or dummy map
# destructor/provider is permitted.
xcrun clang++ -arch arm64 -isysroot "$sdk_root" -Wl,-dead_strip \
  "$task_stage/factory-test.o" "${objects[@]}" \
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
  -o "$task_stage/factory-test"

"$task_stage/factory-test"
