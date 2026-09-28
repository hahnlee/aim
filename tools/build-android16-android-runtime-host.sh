#!/bin/bash
set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/.." && pwd)"
aosp="$project_root/_aosp"
source_root="$aosp/frameworks/base/core/jni"
lock_file="$project_root/upstream/android16-android-runtime-host.lock"
build_dir="$project_root/_build/android-runtime-host"

# shellcheck disable=SC1090
source "$lock_file"

sha256() { shasum -a 256 "$1" | awk '{print $1}'; }
fail() {
  echo "android-runtime-host: $*" >&2
  exit 3
}
verify_hash() {
  local path="$1" expected="$2"
  [[ -f "$path" ]] || fail "missing input: $path"
  local actual
  actual="$(sha256 "$path")"
  [[ "$actual" == "$expected" ]] ||
    fail "checksum mismatch: $path expected=$expected actual=$actual"
}

host_runtime="$source_root/platform/host/HostRuntime.cpp"
android_runtime="$source_root/AndroidRuntime.cpp"
android_runtime_header="$source_root/include/android_runtime/AndroidRuntime.h"
verify_hash "$host_runtime" "$HOST_RUNTIME_CPP_SHA256"
verify_hash "$android_runtime" "$ANDROID_RUNTIME_CPP_SHA256"
verify_hash "$android_runtime_header" "$ANDROID_RUNTIME_H_SHA256"
verify_hash "$project_root/platform/darwin/android_runtime_host.cc" \
  "$DARWIN_ANDROID_RUNTIME_HOST_CPP_SHA256"
verify_hash "$project_root/include/aim/android_runtime_host.h" \
  "$DARWIN_ANDROID_RUNTIME_HOST_H_SHA256"
verify_hash "$project_root/probes/android_runtime_host_ownership_smoke.cc" \
  "$DARWIN_ANDROID_RUNTIME_HOST_PROBE_SHA256"

"$script_dir/build-android16-resource-jni.sh" >/dev/null

stage_parent="$build_dir/stage"
mkdir -p "$stage_parent"
stage="$(mktemp -d "$stage_parent/build.XXXXXX")"
trap 'rm -rf "$stage"' EXIT
objects="$stage/objects"
mkdir -p "$objects"

cxx="$(command -v clang++)"
libtool_bin="$(xcrun --find libtool)"

androidfw="$aosp/frameworks/base/libs/androidfw"
frameworks_native="$aosp/frameworks/native"
system_core="$aosp/system/core"
system_logging="$aosp/system/logging"
libbase="$aosp/system/libbase"
fmtlib="$aosp/external/fmtlib"
nativehelper="$aosp/libnativehelper-full"
icu="$aosp/external/icu/icu4c/source"

flags=(
  -std=gnu++20 -arch arm64 -O2 -fPIC -fno-rtti -fvisibility=hidden
  -DU_USING_ICU_NAMESPACE=0
  -DANDROID_UTILS_REF_BASE_DISABLE_IMPLICIT_CONSTRUCTION
  -Wall -Werror -Wextra -Wthread-safety -Wunused -Wunreachable-code
  -Wno-cast-function-type-mismatch -Wno-unused-parameter
  -Wno-unused-const-variable -Wno-unused-function
  -Wno-unknown-warning-option -Wno-nontrivial-memcall
  -Wno-inconsistent-missing-override -Wno-non-virtual-dtor
  -I"$project_root/include"
  -I"$source_root" -I"$source_root/include"
  -I"$aosp/frameworks/base/libs/hwui/apex/include"
  -I"$aosp/frameworks/base/libs/hwui"
  -I"$androidfw/include" -I"$frameworks_native/include"
  -I"$system_core/libutils/include" -I"$system_core/libutils/binder/include"
  -I"$system_core/libcutils/include" -I"$system_core/libsystem/include"
  -I"$system_logging/liblog/include" -I"$libbase/include" -I"$fmtlib/include"
  -I"$nativehelper/include" -I"$nativehelper/include_platform"
  -I"$nativehelper/include_platform_header_only"
  -I"$nativehelper/header_only_include" -I"$nativehelper/include_jni"
  -I"$icu/common" -I"$icu/i18n"
)

upstream_object="$objects/HostRuntime.o"
"$cxx" "${flags[@]}" -c "$host_runtime" -o "$upstream_object"
[[ "$(file "$upstream_object")" == *"Mach-O 64-bit object arm64"* ]] ||
  fail "upstream HostRuntime is not arm64 Mach-O"
upstream_symbols="$stage/upstream-host-symbols.txt"
nm -gU "$upstream_object" | c++filt > "$upstream_symbols"
grep -F ' T android::AndroidRuntime::getJNIEnv()' "$upstream_symbols" >/dev/null ||
  fail "upstream HostRuntime getJNIEnv definition missing"
grep -F ' T android::AndroidRuntime::getJavaVM()' "$upstream_symbols" >/dev/null ||
  fail "upstream HostRuntime getJavaVM definition missing"
grep -F ' T _JNI_OnLoad' "$upstream_symbols" >/dev/null ||
  fail "upstream HostRuntime JNI_OnLoad definition missing"

upstream_archive="$stage/libandroid-runtime-upstream-host.a"
"$libtool_bin" -static -o "$upstream_archive" "$upstream_object"

resource_archive="$project_root/_build/resource-jni-foundation/libandroid-resource-jni-darwin.a"
[[ -f "$resource_archive" ]] || fail "resource JNI archive missing"
provider_archives=(
  "$project_root/_build/bionic-runtime-provider-closure/libaim-bionic-rust-providers.a"
  "$project_root/_build/bionic-runtime-provider-closure/libaim-bionic-native-providers.a"
  "$project_root/_build/bionic-runtime-provider-closure/libaim-bionic-float-conversion.a"
  "$project_root/_build/androidfw-foundation/libandroidfw-darwin.a"
  "$project_root/_build/nativehelper-device-foundation/libnativehelper-device-darwin.a"
  "$project_root/_build/graphics-foundations/libutils-darwin.a"
  "$project_root/_build/graphics-foundations/libutils-binder-darwin.a"
  "$project_root/_build/graphics-foundations/libcutils-darwin.a"
  "$project_root/_build/graphics-foundations/liblog-darwin.a"
  "$project_root/_build/libbase-foundation/libandroid-base-darwin.a"
  "$project_root/_build/ziparchive-incfs/libziparchive-for-incfs-darwin.a"
  "$project_root/_build/foundation/libziparchive-darwin.a"
  "$project_root/_build/icu-foundation/libicui18n-darwin.a"
  "$project_root/_build/icu-foundation/libicuuc-common-darwin.a"
  "$project_root/_build/icu-foundation/libicuuc-stubdata-darwin.a"
  "$project_root/_build/graphics-codecs/libpng-darwin.a"
  "$project_root/_build/graphics-codecs/libz-darwin.a"
)
for provider in "${provider_archives[@]}"; do
  [[ -f "$provider" ]] || fail "provider archive missing: $provider"
done

exports="$stage/resource.exports"
cat > "$exports" <<'EOF'
__ZN7android38register_android_content_res_ApkAssetsEP7_JNIEnv
__ZN7android37register_android_content_AssetManagerEP7_JNIEnv
__ZN7android36register_android_content_StringBlockEP7_JNIEnv
__ZN7android33register_android_content_XmlBlockEP7_JNIEnv
EOF

# This is an intentional negative gate. HostRuntime.cpp compiles, but its
# unordered_map has a live global constructor. Pulling getJNIEnv from the same
# archive member therefore retains 17 unrelated registrar references even with
# dead stripping enabled. The resource and Bionic provider closures satisfy the
# virtual AssetManager descriptor adoption boundary below.
upstream_link_log="$stage/upstream-host-link.log"
set +e
"$cxx" -arch arm64 -bundle -Wl,-undefined,error -Wl,-dead_strip \
  -Wl,-exported_symbols_list,"$exports" -Wl,-force_load,"$resource_archive" \
  "$upstream_archive" "${provider_archives[@]}" \
  -o "$stage/upstream-host-resource.bundle" 2> "$upstream_link_log"
upstream_status=$?
set -e
[[ "$upstream_status" -ne 0 ]] ||
  fail "upstream HostRuntime unexpectedly became a narrow provider"
upstream_undefined="$stage/upstream-host-unresolved.txt"
sed -n 's/^  "\(.*\)", referenced from:$/\1/p' "$upstream_link_log" |
  sort -u > "$upstream_undefined"
upstream_count="$(wc -l < "$upstream_undefined" | tr -d ' ')"
upstream_sha="$(sha256 "$upstream_undefined")"
if [[ "$upstream_count" != "$UPSTREAM_HOST_UNRESOLVED_COUNT" ||
      "$upstream_sha" != "$UPSTREAM_HOST_UNRESOLVED_SHA256" ]]; then
  cat "$upstream_link_log" >&2
  fail "upstream HostRuntime dead-strip failure drift count=$upstream_count sha=$upstream_sha"
fi

seam_object="$objects/android_runtime_host.o"
"$cxx" "${flags[@]}" \
  -c "$project_root/platform/darwin/android_runtime_host.cc" -o "$seam_object"
[[ "$(file "$seam_object")" == *"Mach-O 64-bit object arm64"* ]] ||
  fail "Darwin AndroidRuntime seam is not arm64 Mach-O"
seam_symbols="$stage/darwin-host-symbols.txt"
nm -gU "$seam_object" | c++filt > "$seam_symbols"
for symbol in \
  'T android::AndroidRuntime::getJNIEnv()' \
  'T android::AndroidRuntime::getJavaVM()' \
  'T _aim_android_runtime_install' \
  'T _aim_android_runtime_uninstall'; do
  grep -F "$symbol" "$seam_symbols" >/dev/null ||
    fail "Darwin ownership symbol missing: $symbol"
done
if nm "$seam_object" | c++filt | grep -F 'AndroidRuntime::mJavaVM' >/dev/null; then
  fail "Darwin ownership seam unexpectedly depends on upstream mJavaVM storage"
fi

seam_archive="$stage/libandroid-runtime-darwin-host.a"
"$libtool_bin" -static -o "$seam_archive" "$seam_object"

probe_object="$objects/android_runtime_host_ownership_smoke.o"
"$cxx" "${flags[@]}" \
  -c "$project_root/probes/android_runtime_host_ownership_smoke.cc" \
  -o "$probe_object"
probe_executable="$stage/android-runtime-host-ownership-smoke"
"$cxx" -arch arm64 "$probe_object" "$seam_archive" -o "$probe_executable"
"$probe_executable" > "$stage/ownership-smoke.log"

closure_bundle="$stage/resource-jni-closure.bundle"
"$cxx" -arch arm64 -bundle -Wl,-undefined,error -Wl,-dead_strip \
  -Wl,-exported_symbols_list,"$exports" -Wl,-force_load,"$resource_archive" \
  "$seam_archive" "${provider_archives[@]}" -o "$closure_bundle"
[[ "$(file "$closure_bundle")" == *"Mach-O 64-bit bundle arm64"* ]] ||
  fail "resource closure is not an arm64 Mach-O bundle"
closure_symbols="$stage/resource-closure-symbols.txt"
nm -aC "$closure_bundle" > "$closure_symbols"
grep -F 'android::AndroidRuntime::getJNIEnv()' "$closure_symbols" >/dev/null ||
  fail "resource closure did not retain genuine Darwin getJNIEnv provider"
if grep -F '_JNI_OnLoad' "$closure_symbols" >/dev/null; then
  fail "resource closure retained upstream HostRuntime JNI_OnLoad"
fi

mkdir -p "$build_dir"
cp "$seam_archive" "$build_dir/libandroid-runtime-darwin-host.a"
cp "$seam_object" "$build_dir/android-runtime-darwin-host.o"
cp "$closure_bundle" "$build_dir/resource-jni-closure.bundle"
cp "$upstream_undefined" "$build_dir/upstream-host-unresolved.txt"
cp "$upstream_link_log" "$build_dir/upstream-host-link.log"
cp "$seam_symbols" "$build_dir/darwin-host-symbols.txt"
cp "$stage/ownership-smoke.log" "$build_dir/ownership-smoke.log"

echo "android-runtime-host: upstream-compile=pass upstream-dead-strip=blocked($upstream_count registrars) ownership-smoke=pass resource-closure=pass arm64=pass"
