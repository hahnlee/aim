#!/bin/bash
set -euo pipefail
export LC_ALL=C
root="$(cd "$(dirname "$0")/.." && pwd)"
source "$root/upstream/android16-system-properties.lock"
source_root="$root/_aosp/android16-system-properties"
out="$root/_build/system-properties"
mkdir -p "$source_root" "$out"
input="$source_root/android_os_SystemProperties.cpp"
if [[ ! -f "$input" ]]; then
  staged="$(mktemp "$source_root/download.XXXXXX")"
  curl -fsSL "https://android.googlesource.com/platform/frameworks/base/+/$FRAMEWORKS_BASE_REVISION/core/jni/android_os_SystemProperties.cpp?format=TEXT" |
    base64 -D > "$staged"
  [[ "$(shasum -a 256 "$staged" | awk '{print $1}')" == "$SYSTEM_PROPERTIES_JNI_SHA256" ]]
  mv "$staged" "$input"
fi
[[ "$(shasum -a 256 "$input" | awk '{print $1}')" == "$SYSTEM_PROPERTIES_JNI_SHA256" ]]
nativehelper="$root/_build/nativehelper-foundation/source/libnativehelper"
helpers="$root/_aosp/frameworks-base-android-util-log/core/jni"
flags=( -std=c++20 -arch arm64 -fPIC -Wno-writable-strings
  -include "$root/tools/system-properties/art_jni_abi.h"
  -I"$source_root" -I"$helpers" -I"$helpers/include"
  -I"$nativehelper/include_jni" -I"$nativehelper/include"
  -I"$nativehelper/include_platform" -I"$nativehelper/header_only_include"
  -I"$root/_aosp/system/libbase/include" -I"$root/_aosp/external/fmtlib/include"
  -I"$root/_aosp/system/core/libutils/include" -I"$root/_aosp/system/core/libsystem/include"
  -I"$root/_aosp/system/logging/liblog/include" )
production_flags=( "${flags[@]}"
  -include "$root/tools/system-properties/production_jni_abi.h"
  -I"$root/tools/bionic-errno-tls/include" )
xcrun clang++ "${production_flags[@]}" -c "$input" \
  -o "$out/android_os_SystemProperties.o"
production_imports="$out/production-undefined.txt"
nm -u "$out/android_os_SystemProperties.o" | sed 's/^ *//' | sort -u > "$production_imports"
if grep -E '^___system_property_(find|read_callback|set)$' "$production_imports" >/dev/null; then
  echo 'system-properties: production JNI retained host property imports' >&2
  exit 2
fi
for symbol in \
  _darwin_art_bionic___system_property_find \
  _darwin_art_bionic___system_property_read_callback \
  _darwin_art_aosp_system_property_set \
  _darwin_art_bionic_errno_to_darwin; do
  grep -Fx "$symbol" "$production_imports" >/dev/null || {
    echo "system-properties: production JNI missing owner import: $symbol" >&2
    exit 2
  }
done
xcrun libtool -static -o "$out/libsystem-properties-jni-darwin.a" "$out/android_os_SystemProperties.o"
xcrun clang++ "${flags[@]}" "$root/tools/system-properties/critical_abi_test.cc" \
  "$root/_build/libbase-foundation/libandroid-base-darwin.a" \
  "$root/_build/graphics-foundations/liblog-darwin.a" \
  -Wl,-dead_strip -o "$out/critical-abi-test"
"$out/critical-abi-test"
echo 'system-properties: original ART JNI compiled with Android-owned property and errno ABI imports'
