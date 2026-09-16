#!/bin/bash
set -euo pipefail
export LC_ALL=C
root="$(cd "$(dirname "$0")/.." && pwd)"
manifest="$root/tools/native-loader-policy/namespace-backend.sources"
out="$root/_build/android-namespace-backend"
objects="$out/objects"
archive="$out/libandroid-namespace-backend.a"
mkdir -p "$objects"

flags=(-std=c++20 -arch arm64 -O2 -fPIC -DART_TARGET_ANDROID -D__ANDROID_API__=36 '-D__INTRODUCED_IN(n)='
  -I"$root/_aosp/frameworks/native/include"
  -I"$root/_aosp/frameworks/native/libs/nativewindow/include"
  -I"$root/_aosp/frameworks/native/libs/arect/include"
  -I"$root/_aosp/system/logging/liblog/include"
  -I"$root/_aosp/bionic-linker-config/linker" -I"$root/_aosp/android16-native-loader-policy"
  -I"$root/_aosp/system/libbase/include" -I"$root/compat" -I"$root/include"
  -I"$root/_aosp/art/libnativeloader/include"
  -I"$root/_aosp/art/libnativebridge/include"
  -I"$root/_aosp/art/libartbase"
  -include "$root/tools/native-loader-policy/darwin_types.h"
  -DABI_STRING='"arm64"'
  -I"$root/_aosp/external/fmtlib/include"
  -I"$root/_build/nativehelper-foundation/source/libnativehelper/include_jni"
  -I"$root/tools/bionic-process-state-facade/include"
  -I"$root/tools/bionic-errno-tls/include"
  -I"$root/tools/bionic-socket-broker-adapter/include"
  -I"$root/tools/android-dl-iterate-phdr-provider/include"
  -I"$root/tools/bionic-dso-lifecycle-facade/include"
  -I"$root/tools/bionic-vm-facade/include"
  -I"$root/tools/android-dso-namespace/include"
  -I"$root/crates/darwin-art-elf-loader/include"
  -I"$root/tools/bionic-provider-namespace/include"
  -I"$root/tools/bionic-provider-namespace/generated")

objects_list=()
object_names=()
while IFS= read -r relative || [[ -n "$relative" ]]; do
  [[ -z "$relative" || "$relative" == \#* ]] && continue
  case "$relative" in
    *native_loader.cpp|*native_loader.cc|*native_bridge.cc|*test.cc|*test.c|*probe.cc|*probe.c|tools/bionic-provider-namespace/src/namespace.cc|compat/darwin_android_elf_image_registry.cc)
      echo "android-namespace-backend: non-production source in manifest: $relative" >&2
      exit 1
      ;;
  esac
  source="$root/$relative"
  [[ -f "$source" ]] || {
    echo "android-namespace-backend: missing manifest source: $relative" >&2
    exit 1
  }
  object_name="$(basename "${relative%.*}").o"
  for existing_name in ${object_names[@]+"${object_names[@]}"}; do
    [[ "$existing_name" != "$object_name" ]] || {
      echo "android-namespace-backend: duplicate object basename: $object_name" >&2
      exit 1
    }
  done
  object_names+=("$object_name")
  object="$objects/$object_name"
  xcrun clang++ "${flags[@]}" -c "$source" -o "$object"
  objects_list+=("$object")
done < "$manifest"

[[ "${#objects_list[@]}" -gt 0 ]] || {
  echo 'android-namespace-backend: empty production source manifest' >&2
  exit 1
}
xcrun libtool -static -o "$archive" "${objects_list[@]}"
xcrun nm -u "$archive" > "$out/required-symbols.txt"
echo "android-namespace-backend: archive=$archive sources=${#objects_list[@]}"
