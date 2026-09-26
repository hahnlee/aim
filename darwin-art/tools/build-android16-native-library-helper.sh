#!/bin/bash
# com.android.internal.content.NativeLibraryHelper (libandroid_runtime):
# PackageManagerService's ABI derivation and native-library extraction.
set -euo pipefail
export LC_ALL=C

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/.." && pwd)"
source "$project_root/upstream/android16-native-library-helper.lock"

aosp="$project_root/_aosp"
source_root="$aosp/frameworks-base-native-library-helper"
build_dir="$project_root/_build/native-library-helper"
patch_file="$project_root/patches/frameworks-base/0021-darwin-native-library-helper.patch"
stage="$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-native-library-helper.XXXXXX")"
trap 'rm -rf "$stage"' EXIT

sha256() { shasum -a 256 "$1" | awk '{print $1}'; }
fail() { echo "native-library-helper: $*" >&2; exit 3; }

materialize() {
  local relative="$1" expected="$2" destination="$source_root/$1"
  if [[ ! -f "$destination" ]]; then
    mkdir -p "$(dirname "$destination")"
    local staged
    staged="$(mktemp "${destination}.download.XXXXXX")"
    curl -fsSL \
      "https://android.googlesource.com/$FRAMEWORKS_BASE_PROJECT/+/$FRAMEWORKS_BASE_REVISION/$relative?format=TEXT" \
      | base64 -D > "$staged"
    [[ "$(sha256 "$staged")" == "$expected" ]] ||
      fail "download checksum mismatch: $relative"
    mv "$staged" "$destination"
  fi
  [[ "$(sha256 "$destination")" == "$expected" ]] ||
    fail "checksum mismatch: $relative"
}

materialize core/jni/Android.bp "$ANDROID_BP_SHA256"
materialize core/jni/com_android_internal_content_NativeLibraryHelper.cpp \
  "$NATIVE_LIBRARY_HELPER_CPP_SHA256"
materialize core/jni/com_android_internal_content_FileSystemUtils.cpp \
  "$FILE_SYSTEM_UTILS_CPP_SHA256"
materialize core/jni/com_android_internal_content_FileSystemUtils.h \
  "$FILE_SYSTEM_UTILS_H_SHA256"
materialize core/jni/core_jni_helpers.h "$CORE_JNI_HELPERS_SHA256"
materialize core/jni/jni_wrappers.h "$JNI_WRAPPERS_SHA256"
materialize core/jni/include/android_runtime/AndroidRuntime.h "$ANDROID_RUNTIME_H_SHA256"
[[ "$(sha256 "$patch_file")" == "$DARWIN_PATCH_SHA256" ]] || fail "Darwin patch changed"

for unit in com_android_internal_content_NativeLibraryHelper.cpp \
    com_android_internal_content_FileSystemUtils.cpp; do
  grep -F "\"$unit\"" "$source_root/core/jni/Android.bp" >/dev/null ||
    fail "Soong owner changed: $unit"
done

patched="$stage/source"
mkdir -p "$patched"
cp -R "$source_root/core" "$patched/core"
patch -d "$patched" -p1 --no-backup-if-mismatch < "$patch_file" >/dev/null ||
  fail "Darwin patch no longer applies"

methods="$(python3 - "$patched/core/jni/com_android_internal_content_NativeLibraryHelper.cpp" <<'PY'
import re
import sys
from pathlib import Path

text = Path(sys.argv[1]).read_text()
start = text.index("static const JNINativeMethod gMethods[]")
end = text.index("\n};", start)
print(len(re.findall(r'\{\s*"([^"]+)"\s*,', text[start:end])))
PY
)"
[[ "$methods" == "$METHOD_COUNT" ]] || fail "method count changed: $methods"

ndk="${ANDROID_NDK_HOME:-$HOME/Library/Android/sdk/ndk/28.2.13676358}"
[[ -d "$ndk/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include" ]] ||
  fail "NDK sysroot (elf.h) missing: $ndk"
nativehelper="$project_root/_build/nativehelper-foundation/source/libnativehelper"
cxx="$(xcrun --find clang++)"
sdk_root="$(xcrun --sdk macosx --show-sdk-path)"
flags=(
  -std=c++20 -arch arm64 -isysroot "$sdk_root" -fPIC -O2
  -DNDEBUG -UDEBUG
  # Darwin's stat/statfs/off_t are already 64-bit.
  -Dstat64=stat -Dlstat64=lstat -Dstatfs64=statfs -Doff64_t=off_t
  -Wall -Werror -Wno-unused-parameter -Wno-vla-cxx-extension -Wno-unused-function
  # Upstream unused locals (Soong builds this unit without -Werror for them).
  -Wno-unused-variable
  -I"$patched/core/jni" -I"$patched/core/jni/include"
  -I"$aosp/frameworks/base/libs/androidfw/include"
  -I"$aosp/system/libziparchive/include"
  -I"$aosp/system/core/libutils/include" -I"$aosp/system/core/libutils/binder/include"
  -I"$aosp/system/core/libcutils/include" -I"$aosp/system/core/libsystem/include"
  -I"$aosp/system/logging/liblog/include" -I"$aosp/system/libbase/include"
  -I"$aosp/external/fmtlib/include" -I"$aosp/system/incremental_delivery/incfs/util/include"
  -I"$nativehelper/include_jni" -I"$nativehelper/include"
  -I"$nativehelper/include_platform" -I"$nativehelper/header_only_include"
  # Darwin has no <elf.h>; use the NDK's Linux ABI definitions.
  -idirafter "$ndk/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include"
  -idirafter "$ndk/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include/aarch64-linux-android"
)
objects=()
for unit in com_android_internal_content_NativeLibraryHelper com_android_internal_content_FileSystemUtils; do
  "$cxx" "${flags[@]}" -c "$patched/core/jni/$unit.cpp" -o "$stage/$unit.o"
  [[ "$(file "$stage/$unit.o")" == *"Mach-O 64-bit object arm64"* ]] || fail "$unit is not Darwin arm64"
  objects+=("$stage/$unit.o")
done
archive="$stage/libandroid-native-library-helper-darwin.a"
"$(xcrun --find libtool)" -static -o "$archive" "${objects[@]}"
nm -gU "$archive" | c++filt > "$stage/definitions.txt"
grep -F ' T android::register_com_android_internal_content_NativeLibraryHelper(_JNIEnv*)' \
  "$stage/definitions.txt" >/dev/null || fail "registrar definition missing"

mkdir -p "$build_dir"
cp "$archive" "$build_dir/libandroid-native-library-helper-darwin.a"
echo "native-library-helper: methods=$methods owner=libandroid_runtime archive=Mach-O-arm64"
