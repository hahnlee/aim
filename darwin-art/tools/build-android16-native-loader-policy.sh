#!/bin/bash
set -euo pipefail
export LC_ALL=C
root="$(cd "$(dirname "$0")/.." && pwd)"
source_root="$root/_aosp/android16-native-loader-policy"
out="$root/_build/native-loader-policy"
mkdir -p "$source_root/android" "$source_root/sys" "$source_root/android-modules-utils" "$out"
while read -r project revision path expected; do
  [[ "$project" != \#* && -n "$project" ]] || continue
  leaf="${path##*/}"
  case "$leaf" in
    dlext.h|api-level.h) leaf="android/$leaf" ;;
    system_properties.h) leaf="sys/$leaf" ;;
    sdk_level.h) leaf="android-modules-utils/$leaf" ;;
  esac
  destination="$source_root/$leaf"
  if [[ ! -f "$destination" ]]; then
    staged="$(mktemp "$source_root/download.XXXXXX")"
    curl -fsSL "https://android.googlesource.com/$project/+/$revision/$path?format=TEXT" |
      base64 -D > "$staged"
    [[ "$(shasum -a 256 "$staged" | awk '{print $1}')" == "$expected" ]]
    mv "$staged" "$destination"
  fi
  [[ "$(shasum -a 256 "$destination" | awk '{print $1}')" == "$expected" ]]
done < "$root/upstream/android16-native-loader-policy.sources"
nativehelper="$root/_build/nativehelper-foundation/source/libnativehelper"
generator="$root/_build/hwui-static-deps/sysprop_cpp"
[[ -x "$generator" ]] || { echo 'Run tools/sync-android16-hwui-static-deps.sh first' >&2; exit 1; }
mkdir -p "$out/generated/include/android/sysprop" "$out/generated/source" "$out/generated/public"
"$generator" --header-dir "$out/generated/include/android/sysprop" \
  --source-dir "$out/generated/source" \
  --include-name android/sysprop/VndkProperties.sysprop.h \
  --public-header-dir "$out/generated/public" "$source_root/VndkProperties.sysprop"
source "$root/upstream/android16-native-loader-policy-generated.lock"
[[ "$(shasum -a 256 "$out/generated/source/VndkProperties.sysprop.cpp" | awk '{print $1}')" == "$VNDK_CPP_SHA256" ]]
[[ "$(shasum -a 256 "$out/generated/include/android/sysprop/VndkProperties.sysprop.h" | awk '{print $1}')" == "$VNDK_HEADER_SHA256" ]]
flags=( -std=c++20 -arch arm64 -fPIC -DART_TARGET_ANDROID -D__ANDROID_API__=36
  -D__system_property_get=darwin_art_bionic___system_property_get
  '-D__INTRODUCED_IN(n)='
  -include "$root/tools/native-loader-policy/darwin_types.h"
  -include "$root/compat/loader/native_loader_libdl.h"
  -I"$source_root"
  -iquote "$root/_aosp/system/logging/liblog/include"
  -iquote "$root/compat" -I"$root/include" -I"$root/tools/bionic-errno-tls/include"
  -I"$root/tools/bionic-process-state-facade/include"
  -I"$root/tools/bionic-fs-facade/include" -I"$root/tools/bionic-ioctl-facade/include"
  -I"$out/generated/include"
  -I"$root/_aosp/art/libnativeloader/include"
  -I"$root/_aosp/art/libartbase"
  -I"$root/_aosp/art/libnativebridge/include"
  -I"$root/_aosp/system/libbase/include"
  -I"$root/_aosp/external/fmtlib/include"
  -I"$root/_aosp/system/logging/liblog/include"
  -I"$nativehelper/include_jni" -I"$nativehelper/header_only_include" )
# Compile the device policy, never ART's host null-success implementation.
# Backend and public-library configuration must link before production use.
mkdir -p "$out/source"
[[ "$(shasum -a 256 "$root/patches/native-loader-policy/guest-config.patch" | awk '{print $1}')" == 48e60e08d9e48bef00c3922898246cdcaf0e7f436767c04d7bc0417a81f4be75 ]]
cp "$source_root/public_libraries.cpp" "$out/source/public_libraries.cpp"
patch --batch --fuzz=0 -d "$out/source" -p1 < "$root/patches/native-loader-policy/guest-config.patch"
[[ "$(shasum -a 256 "$out/source/public_libraries.cpp" | awk '{print $1}')" == df5be0c97693321a1aa8e0c1a28ae2325e230de6a06fcaa25b3b026636cd7485 ]]
for unit in library_namespaces native_loader_namespace public_libraries; do
  unit_source="$source_root/$unit.cpp"
  if [[ "$unit" == public_libraries ]]; then unit_source="$out/source/$unit.cpp"; fi
  xcrun clang++ "${flags[@]}" -c "$unit_source" -o "$out/$unit.o"
done
# Compile the real lifecycle owner as well as its policy helpers. Keep this
# archive out of production until its namespace backend replaces the existing
# loader exports; linking both would retain conflicting process ownership.
loader_source="$root/_aosp/android16-classloader-native-state/libnativeloader/native_loader.cpp"
[[ "$(shasum -a 256 "$loader_source" | awk '{print $1}')" == cb7e74e163427c8a04c6113d7f3c53dc2b789befbfbcd19ffdea57da5c6da01d ]]
# Preserve the upstream device policy, but resolve its Android file probes in
# the installed guest filesystem, never the host's /system directory. Use the
# Android stat layout explicitly; redirecting stat on a Darwin struct is unsafe.
[[ "$(shasum -a 256 "$root/patches/native-loader-policy/guest-loader-stat.patch" | awk '{print $1}')" == 724ccd84924589d7e03689edc8164ac137c2f6091cad6998591512cf9bffe2ce ]]
cp "$loader_source" "$out/source/native_loader.cpp"
patch --batch --fuzz=0 -d "$out/source" -p1 < "$root/patches/native-loader-policy/guest-loader-stat.patch"
[[ "$(shasum -a 256 "$out/source/native_loader.cpp" | awk '{print $1}')" == 80cf372f7642c374a9f2bca415aafb154ccbad970871fc8d695c3d94ee182cda ]]
xcrun clang++ "${flags[@]}" -c "$out/source/native_loader.cpp" -o "$out/native_loader.o"
helpers="$root/_aosp/frameworks-base-android-util-log/core/jni"
xcrun clang++ "${flags[@]}" -Wno-writable-strings \
  -I"$helpers" -I"$helpers/include" \
  -I"$nativehelper/include" -I"$nativehelper/include_platform" \
  -I"$root/_aosp/system/core/libutils/include" \
  -I"$root/_aosp/system/core/libsystem/include" \
  -c "$source_root/com_android_internal_os_ClassLoaderFactory.cpp" \
  -o "$out/classloader_factory_jni.o"
xcrun clang++ "${flags[@]}" -c "$root/compat/filesystem/guest_config.cc" -o "$out/guest_config.o"
xcrun clang++ "${flags[@]}" -c "$root/compat/filesystem/guest_file.cc" -o "$out/guest_file.o"
xcrun clang++ "${flags[@]}" -c "$root/compat/filesystem/guest_directory.cc" -o "$out/guest_directory.o"
xcrun clang++ "${flags[@]}" -c "$root/compat/filesystem/linker_config_fs.cc" -o "$out/linker_config_fs.o"
xcrun clang++ "${flags[@]}" -c "$root/compat/process/native_loader_sdk.cc" -o "$out/native_loader_sdk.o"
# Keep libbase's original device implementation: never compile its host map.
[[ "$(shasum -a 256 "$root/_aosp/system/libbase/properties.cpp" | awk '{print $1}')" == 039e7dbaaecc97610a24604f4d1e0a3cb5172a68c71522ff48dd619e84a57c94 ]]
property_flags=( -D__BIONIC__ )
for symbol in find read_callback wait serial area_serial; do
  property_flags+=( "-D__system_property_${symbol}=darwin_art_bionic___system_property_${symbol}" )
done
property_flags+=( -D__system_property_set=darwin_art_aosp_system_property_set )
xcrun clang++ "${flags[@]}" "${property_flags[@]}" \
  -include "$root/tools/native-loader-policy/device_property_declarations.h" \
  -c "$root/_aosp/system/libbase/properties.cpp" -o "$out/device_properties.o"
xcrun nm -u "$out/device_properties.o" | sed 's/^[[:space:]]*//' | sort -u \
  > "$out/device-properties-required.txt"
if grep -E '^_darwin_art_bionic___system_property_set$|^___system_property_' \
    "$out/device-properties-required.txt" >/dev/null; then
  echo 'native-loader-policy: device libbase retained an unowned property setter/host import' >&2
  exit 1
fi
for symbol in find read_callback wait serial area_serial; do
  grep -Fx "_darwin_art_bionic___system_property_${symbol}" \
    "$out/device-properties-required.txt" >/dev/null || {
    echo "native-loader-policy: device libbase missing property owner import: $symbol" >&2
    exit 1
  }
done
grep -Fx '_darwin_art_aosp_system_property_set' "$out/device-properties-required.txt" >/dev/null || {
  echo 'native-loader-policy: device libbase missing Android property-client setter import' >&2
  exit 1
}
xcrun clang++ "${flags[@]}" -c "$out/generated/source/VndkProperties.sysprop.cpp" \
  -o "$out/VndkProperties.o"
xcrun libtool -static -o "$out/libnative-loader-policy-darwin.a" \
  "$out/native_loader.o" \
  "$out/classloader_factory_jni.o" \
  "$out/library_namespaces.o" "$out/native_loader_namespace.o" \
  "$out/public_libraries.o" "$out/VndkProperties.o" "$out/guest_config.o" "$out/native_loader_sdk.o" \
  "$out/device_properties.o" "$out/linker_config_fs.o" "$out/guest_directory.o" "$out/guest_file.o"
xcrun nm -gU "$out/library_namespaces.o" > "$out/policy-defined.txt"
xcrun nm -u "$out/public_libraries.o" > "$out/public-libraries-required.txt"
if grep -E ' _(opendir|readdir|closedir)$' "$out/public-libraries-required.txt" >/dev/null; then
  echo 'native-loader-policy: directory search escaped to host DIR APIs' >&2
  exit 1
fi
grep -q 'GuestDirectory4Open' "$out/public-libraries-required.txt"
grep -q 'GuestDirectory4Next' "$out/public-libraries-required.txt"
xcrun nm -u "$out/native_loader_namespace.o" > "$out/backend-required.txt"
xcrun nm -gU "$out/native_loader.o" > "$out/loader-defined.txt"
xcrun nm -u "$out/native_loader.o" > "$out/loader-required.txt"
grep -q '_darwin_art_linker_dlclose$' "$out/loader-required.txt"
grep -q '_darwin_art_linker_dlerror$' "$out/loader-required.txt"
grep -q 'GuestLinkerStat' "$out/loader-required.txt"
if grep -E '(^|[[:space:]])_(stat|stat64|lstat|lstat64)(\$INODE64)?$' "$out/loader-required.txt"; then
  echo 'native-loader-policy: Android file probes escaped to macOS stat' >&2; exit 1
fi
if grep -E '(^|[[:space:]])_(dlopen|dlclose|dlerror)$' "$out/loader-required.txt" "$out/backend-required.txt"; then
  echo 'native-loader-policy: Android libdl escaped to macOS symbols' >&2; exit 1
fi
xcrun nm -u "$out/classloader_factory_jni.o" > "$out/jni-required.txt"
grep -q '_CreateClassLoaderNamespace$' "$out/jni-required.txt"
xcrun nm -gU "$out/classloader_factory_jni.o" > "$out/jni-defined.txt"
grep -q 'register_com_android_internal_os_ClassLoaderFactory' "$out/jni-defined.txt"
for symbol in CreateClassLoaderNamespace InitializeNativeLoader ResetNativeLoader OpenNativeLibrary; do
  grep -q "_$symbol$" "$out/loader-defined.txt"
done
# Empty host-branch objects must not pass as the device implementation.
grep -q 'LibraryNamespaces6Create' "$out/policy-defined.txt"
for symbol in android_create_namespace android_link_namespaces android_dlopen_ext \
              NativeBridgeCreateNamespace NativeBridgeLoadLibraryExt; do
  grep -q "_$symbol$" "$out/backend-required.txt"
done
xcrun clang++ "${flags[@]}" "$root/tools/native-loader-policy/public_config_test.cc" \
  "$out/public_libraries.o" \
  "$root/_build/libbase-foundation/libandroid-base-darwin.a" \
  "$root/_build/graphics-foundations/liblog-darwin.a" \
  -Wl,-dead_strip -o "$out/public-config-test"
"$out/public-config-test"
xcrun clang -std=c11 -I"$root/tools/bionic-errno-tls/include" \
  -I"$root/tools/bionic-errno-tls/generated" \
  -c "$root/tools/bionic-errno-tls/src/errno_tls.c" -o "$out/config-test-errno.o"
xcrun clang++ "${flags[@]}" -D__BIONIC__ "$root/tools/native-loader-policy/guest_config_test.cc" \
  "$out/public_libraries.o" "$out/guest_config.o" "$out/VndkProperties.o" "$out/native_loader_sdk.o" \
  "$out/device_properties.o" "$out/linker_config_fs.o" "$out/guest_directory.o" "$out/guest_file.o" \
  "$out/config-test-errno.o" "$root/_build/libbase-foundation/libandroid-base-darwin.a" \
  "$root/_build/bionic-runtime-provider-closure/libdarwin-art-bionic-native-providers.a" \
  "$root/_build/bionic-runtime-provider-closure/libdarwin-art-bionic-rust-providers.a" \
  "$root/_build/graphics-foundations/liblog-darwin.a" \
  -Wl,-dead_strip -framework Security -lresolv -o "$out/guest-config-test"
"$out/guest-config-test"
xcrun nm -a "$out/guest-config-test" > "$out/guest-config-symbols.txt"
if grep -q 'g_properties' "$out/guest-config-symbols.txt"; then
  echo 'native-loader-policy: host property store leaked into guest policy test' >&2
  exit 1
fi
for symbol in get find read_callback wait serial area_serial; do
  grep -q "_darwin_art_bionic___system_property_${symbol}$" \
    "$out/guest-config-symbols.txt" || {
    echo "native-loader-policy: guest property executable missing owner symbol: $symbol" >&2
    exit 1
  }
done
echo 'native-loader-policy: original default+extension library policy + guest filesystem + bionic SDK PASS'
echo 'native-loader-policy: original device policy compiled; backend linkage pending'
