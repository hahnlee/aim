#!/bin/bash
set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/.." && pwd)"
aosp="$project_root/_aosp"
source_root="$aosp/frameworks/base/core/jni"
lock_file="$project_root/upstream/android16-resource-jni.lock"
critical_patch="$project_root/patches/frameworks-base/0004-darwin-core-critical-jni-abi.patch"
asset_fd_patch="$project_root/patches/frameworks-base/0005-darwin-virtual-asset-fd.patch"
build_dir="$project_root/_build/resource-jni-foundation"

# shellcheck disable=SC1090
source "$lock_file"

sha256() { shasum -a 256 "$1" | awk '{print $1}'; }
fail_missing() {
  echo "resource-jni: missing revision-locked input: $1" >&2
  echo "project=$FRAMEWORKS_BASE_PROJECT revision=$FRAMEWORKS_BASE_REVISION" >&2
  echo "run: $script_dir/materialize-android16-resource-jni.sh" >&2
  exit 2
}
verify_hash() {
  local path="$1" expected="$2"
  [[ -f "$path" ]] || fail_missing "$path"
  local actual
  actual="$(sha256 "$path")"
  if [[ "$actual" != "$expected" ]]; then
    echo "resource-jni: checksum mismatch: $path" >&2
    echo "expected=$expected actual=$actual" >&2
    exit 3
  fi
}

verify_hash "$source_root/Android.bp" "$ANDROID_BP_SHA256"
verify_hash "$source_root/AndroidRuntime.cpp" "$ANDROID_RUNTIME_CPP_SHA256"
verify_hash "$source_root/android_content_res_ApkAssets.cpp" "$APK_ASSETS_CPP_SHA256"
verify_hash "$source_root/android_content_res_ApkAssets.h" "$APK_ASSETS_H_SHA256"
verify_hash "$source_root/android_util_AssetManager.cpp" "$ASSET_MANAGER_CPP_SHA256"
verify_hash "$source_root/android_util_StringBlock.cpp" "$STRING_BLOCK_CPP_SHA256"
verify_hash "$source_root/android_util_XmlBlock.cpp" "$XML_BLOCK_CPP_SHA256"
verify_hash "$source_root/core_jni_helpers.h" "$CORE_JNI_HELPERS_SHA256"
verify_hash "$source_root/jni_wrappers.h" "$JNI_WRAPPERS_SHA256"
verify_hash "$source_root/include/android_runtime/AndroidRuntime.h" "$ANDROID_RUNTIME_H_SHA256"
verify_hash "$source_root/include/android_runtime/android_util_AssetManager.h" \
  "$ANDROID_UTIL_ASSET_MANAGER_H_SHA256"
verify_hash "$critical_patch" "$CRITICAL_JNI_PATCH_SHA256"
verify_hash "$asset_fd_patch" "$ASSET_FD_PATCH_SHA256"

stage_parent="$build_dir/stage"
mkdir -p "$stage_parent"
stage="$(mktemp -d "$stage_parent/build.XXXXXX")"
trap 'rm -rf "$stage"' EXIT
patched="$stage/source"
mkdir -p "$patched/include/android_runtime"
for relative in \
  Android.bp \
  AndroidRuntime.cpp \
  android_content_res_ApkAssets.cpp \
  android_content_res_ApkAssets.h \
  android_util_AssetManager.cpp \
  android_util_StringBlock.cpp \
  android_util_XmlBlock.cpp \
  core_jni_helpers.h \
  jni_wrappers.h; do
  cp "$source_root/$relative" "$patched/$relative"
done
cp "$source_root/include/android_runtime/AndroidRuntime.h" \
  "$patched/include/android_runtime/AndroidRuntime.h"
cp "$source_root/include/android_runtime/android_util_AssetManager.h" \
  "$patched/include/android_runtime/android_util_AssetManager.h"
patch -s -d "$patched" -p1 < "$critical_patch"
patch -s -d "$patched" -p1 < "$asset_fd_patch"
verify_hash "$patched/core_jni_helpers.h" "$CORE_JNI_HELPERS_PATCHED_SHA256"

sources_file="$stage/resource-jni-sources.txt"
python3 - "$patched/Android.bp" > "$sources_file" <<'PY'
import re
import sys
from pathlib import Path

text = Path(sys.argv[1]).read_text()
module = text.index('name: "libandroid_runtime"')
start = text.index('srcs: [', module) + len('srcs: [')
end = text.index('],', start)
wanted = {
    'android_content_res_ApkAssets.cpp',
    'android_util_AssetManager.cpp',
    'android_util_StringBlock.cpp',
    'android_util_XmlBlock.cpp',
}
for source in re.findall(r'"([^"]+)"', text[start:end]):
    if source in wanted:
        print(source)
PY
source_count="$(wc -l < "$sources_file" | tr -d ' ')"
source_sha="$(sha256 "$sources_file")"
if [[ "$source_count" != "$RESOURCE_JNI_SOURCE_COUNT" ||
      "$source_sha" != "$RESOURCE_JNI_SOURCE_LIST_SHA256" ]]; then
  echo "resource-jni: Android.bp resource slice changed" >&2
  echo "count=$source_count sha256=$source_sha" >&2
  exit 3
fi

registrar_order="$stage/resource-registrar-order.txt"
python3 - "$patched/AndroidRuntime.cpp" > "$registrar_order" <<'PY'
import re
import sys
from pathlib import Path

text = Path(sys.argv[1]).read_text()
start = text.index('static const RegJNIRec gRegJNI[]')
end = text.index('};', start)
wanted = {
    'register_android_content_AssetManager',
    'register_android_content_StringBlock',
    'register_android_content_XmlBlock',
    'register_android_content_res_ApkAssets',
}
for registrar in re.findall(r'REG_JNI\(([^)]+)\)', text[start:end]):
    if registrar in wanted:
        print(registrar)
PY
registrar_order_count="$(wc -l < "$registrar_order" | tr -d ' ')"
registrar_order_sha="$(sha256 "$registrar_order")"
if [[ "$registrar_order_count" != 4 ||
      "$registrar_order_sha" != "$RESOURCE_REGISTRAR_ORDER_SHA256" ]]; then
  echo "resource-jni: AndroidRuntime gRegJNI resource order changed" >&2
  exit 3
fi

python3 - "$patched" \
  "$ASSET_MANAGER_METHOD_COUNT" "$APK_ASSETS_METHOD_COUNT" \
  "$STRING_BLOCK_METHOD_COUNT" "$XML_BLOCK_METHOD_COUNT" <<'PY'
import re
import sys
from pathlib import Path

root = Path(sys.argv[1])
expected = dict(zip([
    'android_util_AssetManager.cpp',
    'android_content_res_ApkAssets.cpp',
    'android_util_StringBlock.cpp',
    'android_util_XmlBlock.cpp',
], map(int, sys.argv[2:])))
for name, count in expected.items():
    text = (root / name).read_text()
    match = re.search(r'static const JNINativeMethod g[^[]+\[\] = \{(.*?)\n\};', text, re.S)
    actual = len(re.findall(r'\{\s*"', match.group(1))) if match else -1
    if actual != count:
        raise SystemExit(f'{name}: native method count expected={count} actual={actual}')
PY

androidfw="$aosp/frameworks/base/libs/androidfw"
frameworks_native="$aosp/frameworks/native"
system_core="$aosp/system/core"
system_logging="$aosp/system/logging"
libbase="$aosp/system/libbase"
fmtlib="$aosp/external/fmtlib"
incfs="$aosp/system/incremental_delivery/incfs"
nativehelper="$aosp/libnativehelper-full"

for required in \
  "$androidfw/include/androidfw/AssetManager2.h" \
  "$frameworks_native/include/ftl/static_vector.h" \
  "$system_core/libutils/include/utils/String8.h" \
  "$system_logging/liblog/include/log/log.h" \
  "$libbase/include/android-base/logging.h" \
  "$fmtlib/include/fmt/format.h" \
  "$nativehelper/include/nativehelper/JNIHelp.h"; do
  [[ -f "$required" ]] || fail_missing "$required"
done

cxx="$(command -v clang++)"
libtool_bin="$(xcrun --find libtool)"
ld_bin="$(xcrun --find ld)"
sdk_root="$(xcrun --sdk macosx --show-sdk-path)"
sdk_version="$(xcrun --sdk macosx --show-sdk-version)"
objects="$stage/objects"
mkdir -p "$objects"
flags=(
  -std=gnu++20 -arch arm64 -O2 -fPIC -fno-rtti -fvisibility=hidden
  -DDARWIN_ART_ANDROID_CRITICAL_JNI_ABI
  -DANDROID_UTILS_REF_BASE_DISABLE_IMPLICIT_CONSTRUCTION
  -DU_USING_ICU_NAMESPACE=0
  -Wall -Werror -Wextra -Wthread-safety -Wunused -Wunreachable-code
  -Wno-cast-function-type-mismatch -Wno-unused-parameter
  -Wno-unknown-warning-option
  -Wno-nontrivial-memcall -Wno-inconsistent-missing-override
  -Wno-non-virtual-dtor -Wno-maybe-uninitialized -Wno-parentheses
  -Wno-error=deprecated-declarations -Wno-unused-const-variable
  -Wno-unused-function -Wno-conversion-null
  -I"$patched" -I"$patched/include"
  -I"$androidfw/include" -I"$frameworks_native/include"
  -I"$frameworks_native/libs/arect/include"
  -I"$system_core/libutils/include" -I"$system_core/libutils/binder/include"
  -I"$system_core/libcutils/include" -I"$system_core/libsystem/include"
  -I"$system_logging/liblog/include" -I"$libbase/include" -I"$fmtlib/include"
  -I"$incfs/util/include"
  -I"$nativehelper/include" -I"$nativehelper/include_platform"
  -I"$nativehelper/include_platform_header_only"
  -I"$aosp/libnativehelper/header_only_include"
  -I"$aosp/libnativehelper/include_jni"
)

compiled=()
while IFS= read -r source; do
  object="$objects/${source%.cpp}.o"
  echo "resource-jni: compile $source"
  "$cxx" "${flags[@]}" -c "$patched/$source" -o "$object"
  [[ "$(file "$object")" == *"Mach-O 64-bit object arm64"* ]] || {
    echo "resource-jni: non-arm64 object: $object" >&2
    exit 3
  }
  compiled+=("$object")
done < "$sources_file"

archive="$stage/libandroid-resource-jni-darwin.a"
"$libtool_bin" -static -o "$archive" "${compiled[@]}"
member_count="$({ ar -t "$archive" || true; } | grep -v '^__\.SYMDEF' | wc -l | tr -d ' ')"
[[ "$member_count" == "$RESOURCE_JNI_SOURCE_COUNT" ]] || {
  echo "resource-jni: archive member count=$member_count expected=$RESOURCE_JNI_SOURCE_COUNT" >&2
  exit 3
}

combined="$stage/android-resource-jni-force-loaded.o"
"$ld_bin" -r -arch arm64 -platform_version macos "$sdk_version" "$sdk_version" \
  -syslibroot "$sdk_root" -force_load "$archive" -o "$combined"
definitions="$stage/definitions.txt"
nm -gU "$archive" | c++filt > "$definitions"
for symbol in \
  'android::register_android_content_res_ApkAssets(_JNIEnv*)' \
  'android::register_android_content_AssetManager(_JNIEnv*)' \
  'android::register_android_content_StringBlock(_JNIEnv*)' \
  'android::register_android_content_XmlBlock(_JNIEnv*)'; do
  grep -F " T $symbol" "$definitions" >/dev/null || {
    echo "resource-jni: registrar missing: $symbol" >&2
    exit 3
  }
done

apk_symbols="$stage/apk-assets-symbols.txt"
nm -aC "$objects/android_content_res_ApkAssets.o" > "$apk_symbols"
grep -F 'android::NativeIsUpToDate(long long)' "$apk_symbols" >/dev/null || {
  echo "resource-jni: CriticalNative ABI is not Android-shaped" >&2
  exit 3
}
if grep -F 'android::NativeIsUpToDate(_JNIEnv*' "$apk_symbols" >/dev/null; then
  echo "resource-jni: CriticalNative unexpectedly includes host JNI arguments" >&2
  exit 3
fi

probe="$objects/registrar-smoke.o"
"$cxx" -std=c++20 -arch arm64 -fPIC -I"$nativehelper/include" \
  -I"$aosp/libnativehelper/include_jni" \
  -c "$project_root/probes/android16_resource_jni_registrar_smoke.cpp" -o "$probe"

undefined="$stage/undefined-symbols.txt"
nm -u "$combined" | awk '$1 ~ /^_/ { print $1 }' | sort -u > "$undefined"
undefined_count="$(wc -l < "$undefined" | tr -d ' ')"
undefined_sha="$(sha256 "$undefined")"
if [[ "$undefined_count" != "$UNDEFINED_SYMBOL_COUNT" ||
      "$undefined_sha" != "$UNDEFINED_SYMBOLS_SHA256" ]]; then
  echo "resource-jni: undefined-symbol manifest drift" >&2
  echo "count=$undefined_count sha256=$undefined_sha" >&2
  exit 3
fi

"$script_dir/build-android16-nativehelper-device-foundation.sh" >/dev/null
provider_archives=(
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
  [[ -f "$provider" ]] || fail_missing "$provider"
done
closure_log="$stage/provider-closure.log"
set +e
"$cxx" -arch arm64 -bundle -Wl,-undefined,error \
  -Wl,-force_load,"$archive" "${provider_archives[@]}" \
  -o "$stage/resource-jni-closure.bundle" 2> "$closure_log"
closure_status=$?
set -e
if [[ "$closure_status" -eq 0 ]]; then
  provider_blocker=none
else
  quoted_undefined="$stage/provider-undefined.txt"
  sed -n 's/^  "\(.*\)", referenced from:$/\1/p' "$closure_log" > "$quoted_undefined"
  blocker_count="$(wc -l < "$quoted_undefined" | tr -d ' ')"
  if [[ "$blocker_count" != 3 ]] ||
     ! grep -Fx 'android::AndroidRuntime::getJNIEnv()' "$quoted_undefined" >/dev/null ||
     ! grep -Fx '_darwin_art_bionic_fs_adopt_host_fd_core' "$quoted_undefined" >/dev/null ||
     ! grep -Fx '_darwin_art_bionic_fs_dup_host_fd_core' "$quoted_undefined" >/dev/null; then
    echo "resource-jni: unexpected provider closure failure" >&2
    cat "$closure_log" >&2
    exit 3
  fi
  if ! grep -Fx "$EXPECTED_PROVIDER_BLOCKER" "$undefined" >/dev/null; then
    echo "resource-jni: expected mangled provider blocker missing" >&2
    exit 3
  fi
  provider_blocker='AndroidRuntime::getJNIEnv()+virtual-asset-fd'
fi

mkdir -p "$build_dir"
cp "$archive" "$build_dir/libandroid-resource-jni-darwin.a"
cp "$combined" "$build_dir/android-resource-jni-force-loaded.o"
cp "$sources_file" "$build_dir/resource-jni-sources.txt"
cp "$registrar_order" "$build_dir/resource-registrar-order.txt"
cp "$undefined" "$build_dir/undefined-symbols.txt"
cp "$definitions" "$build_dir/definitions.txt"
cp "$closure_log" "$build_dir/provider-closure.log"

echo "resource-jni: sources=$source_count methods=$((ASSET_MANAGER_METHOD_COUNT + APK_ASSETS_METHOD_COUNT + STRING_BLOCK_METHOD_COUNT + XML_BLOCK_METHOD_COUNT)) archive-members=$member_count registrars=4 critical-jni=android unresolved=$undefined_count provider-blocker=$provider_blocker"
