#!/bin/bash
set -euo pipefail
export LC_ALL=C
root="$(cd "$(dirname "$0")/.." && pwd)"
source "$root/upstream/android16-application-shared-memory.lock"
source_root="$root/_aosp/android16-application-shared-memory"
out="$root/_build/application-shared-memory"
mkdir -p "$source_root" "$out"
sha256() { shasum -a 256 "$1" | awk '{print $1}'; }
materialize() {
  local name="$1" expected="$2" destination="$source_root/$1"
  if [[ ! -f "$destination" ]]; then
    local staged
    staged="$(mktemp "$source_root/download.XXXXXX")"
    if ! curl -fsSL "https://android.googlesource.com/$FRAMEWORKS_BASE_PROJECT/+/$FRAMEWORKS_BASE_REVISION/core/jni/$name?format=TEXT" | base64 -D > "$staged"; then
      rm -f "$staged"
      return 1
    fi
    if [[ "$(sha256 "$staged")" != "$expected" ]]; then
      rm -f "$staged"
      echo "application-shared-memory: source hash mismatch: $name" >&2
      return 1
    fi
    mv "$staged" "$destination"
  fi
  [[ "$(sha256 "$destination")" == "$expected" ]]
}
materialize com_android_internal_os_ApplicationSharedMemory.cpp "$APPLICATION_SHARED_MEMORY_SHA256"
materialize android_app_PropertyInvalidatedCache.h "$PROPERTY_CACHE_HEADER_SHA256"
materialize android_app_PropertyInvalidatedCache.cpp "$PROPERTY_CACHE_SOURCE_SHA256"
patch_file="$root/patches/application-shared-memory/0001-darwin-region-capabilities.patch"
[[ "$(sha256 "$patch_file")" == "$DARWIN_REGION_PATCH_SHA256" ]]
patched="$out/source"
mkdir -p "$patched"
cp "$source_root/com_android_internal_os_ApplicationSharedMemory.cpp" "$patched/"
patch --batch --forward -p1 -d "$patched" < "$patch_file"

# Reuse the same locked JNI support headers as the existing framework owner.
source "$root/upstream/android16-android-util-log.lock"
helpers="$root/_aosp/frameworks-base-android-util-log/core/jni"
[[ "$(sha256 "$helpers/core_jni_helpers.h")" == "$CORE_JNI_HELPERS_SHA256" ]]
[[ "$(sha256 "$helpers/include/android_runtime/AndroidRuntime.h")" == "$ANDROID_RUNTIME_H_SHA256" ]]
nativehelper="$root/_build/nativehelper-foundation/source/libnativehelper"
[[ "$(sha256 "$nativehelper/include/nativehelper/JNIHelp.h")" == "$JNI_HELP_H_SHA256" ]]
flags=( -std=c++20 -arch arm64 -fPIC -Wno-writable-strings
  -I"$patched" -I"$source_root" -iquote "$root/compat" -I"$helpers" -I"$helpers/include"
  -I"$nativehelper/include_jni" -I"$nativehelper/include"
  -I"$nativehelper/include_platform" -I"$nativehelper/header_only_include"
  -I"$root/_aosp/system/logging/liblog/include"
  -I"$root/_aosp/system/core/libutils/include"
  -I"$root/_aosp/system/core/libsystem/include"
  -I"$root/_aosp/system/core/libcutils/include"
  -I"$root/_aosp/system/libbase/include" )
for unit in com_android_internal_os_ApplicationSharedMemory android_app_PropertyInvalidatedCache; do
  input="$source_root/$unit.cpp"
  [[ "$unit" != com_android_internal_os_ApplicationSharedMemory ]] || input="$patched/$unit.cpp"
  xcrun clang++ "${flags[@]}" -c "$input" -o "$out/$unit.o"
done
for unit in system_region application_memory application_descriptor; do
  xcrun clang++ "${flags[@]}" -c "$root/compat/memory/$unit.cc" -o "$out/$unit.o"
done
xcrun libtool -static -o "$out/libapplication-shared-memory-darwin.a" \
  "$out/com_android_internal_os_ApplicationSharedMemory.o" \
  "$out/android_app_PropertyInvalidatedCache.o" \
  "$out/system_region.o" "$out/application_memory.o" "$out/application_descriptor.o"
echo "application-shared-memory: AOSP JNI + Darwin memory backend built (runtime registration required)"
xcrun clang++ "${flags[@]}" -Wl,-dead_strip \
  "$root/tools/application-shared-memory-layout-test.cc" \
  "$root/compat/memory/system_region.cc" \
  "$root/compat/memory/application_memory.cc" \
  "$root/compat/memory/application_descriptor.cc" \
  "$out/android_app_PropertyInvalidatedCache.o" \
  "$root/_build/graphics-foundations/liblog-darwin.a" \
  -o "$out/layout-test"
"$out/layout-test"
xcrun clang++ -std=c++20 -Wall -Wextra -Werror \
  "$root/tools/system-region-test.cc" "$root/compat/memory/system_region.cc" \
  -o "$out/system-region-test"
"$out/system-region-test"
