#!/bin/bash
set -euo pipefail
export LC_ALL=C

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/.." && pwd)"
source "$project_root/upstream/android16-virtual-ref-base-ptr.lock"

source_root="$project_root/_aosp/frameworks-base-virtual-ref-base-ptr"
build_dir="$project_root/_build/virtual-ref-base-ptr"
stage="$(mktemp -d "${TMPDIR:-/tmp}/aim-virtual-ref.XXXXXX")"
trap 'rm -rf "$stage"' EXIT

sha256() { shasum -a 256 "$1" | awk '{print $1}'; }
fail() { echo "virtual-ref-base-ptr: $*" >&2; exit 3; }

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
materialize core/jni/com_android_internal_util_VirtualRefBasePtr.cpp "$SOURCE_SHA256"
materialize core/jni/core_jni_helpers.h "$CORE_JNI_HELPERS_SHA256"
materialize core/jni/jni_wrappers.h "$JNI_WRAPPERS_SHA256"
materialize core/jni/include/android_runtime/AndroidRuntime.h \
  "$ANDROID_RUNTIME_H_SHA256"

grep -F '"com_android_internal_util_VirtualRefBasePtr.cpp"' \
  "$source_root/core/jni/Android.bp" >/dev/null || fail "Soong owner changed"

manifest="$stage/methods.tsv"
python3 - "$source_root/core/jni/com_android_internal_util_VirtualRefBasePtr.cpp" \
  > "$manifest" <<'PY'
import re
import sys
from pathlib import Path

text = Path(sys.argv[1]).read_text()
start = text.index("static const JNINativeMethod gMethods[]")
end = text.index("\n};", start)
for name, signature in re.findall(r'\{\s*"([^"]+)"\s*,\s*"([^"]+)"', text[start:end]):
    print(f"{name}\t{signature}")
PY
[[ "$(wc -l < "$manifest" | tr -d ' ')" == "$METHOD_COUNT" ]] ||
  fail "method count changed"
[[ "$(sha256 "$manifest")" == "$METHOD_MANIFEST_SHA256" ]] ||
  fail "method manifest changed"

nativehelper="$project_root/_build/nativehelper-foundation/source/libnativehelper"
source_file="$source_root/core/jni/com_android_internal_util_VirtualRefBasePtr.cpp"
object="$stage/virtual-ref-base-ptr.o"
archive="$stage/libandroid-virtual-ref-base-ptr-darwin.a"
cxx="$(xcrun --find clang++)"
sdk_root="$(xcrun --sdk macosx --show-sdk-path)"
"$cxx" -std=c++20 -arch arm64 -isysroot "$sdk_root" -fPIC \
  -Wall -Wextra -Werror \
  -Wno-unused-parameter -Wno-writable-strings \
  -I"$source_root/core/jni" \
  -I"$source_root/core/jni/include" \
  -I"$nativehelper/include_jni" -I"$nativehelper/include" \
  -I"$nativehelper/include_platform" -I"$nativehelper/header_only_include" \
  -I"$project_root/_aosp/system/core/libutils/include" \
  -I"$project_root/_aosp/system/core/libsystem/include" \
  -I"$project_root/_aosp/system/libbase/include" \
  -I"$project_root/_aosp/system/logging/liblog/include" \
  -c "$source_file" -o "$object"
"$(xcrun --find libtool)" -static -o "$archive" "$object"

[[ "$(file "$object")" == *"Mach-O 64-bit object arm64"* ]] ||
  fail "object is not Darwin arm64"
definitions="$stage/definitions.txt"
nm -gU "$archive" | c++filt > "$definitions"
grep -F ' T android::register_com_android_internal_util_VirtualRefBasePtr(_JNIEnv*)' \
  "$definitions" >/dev/null || fail "registrar definition missing"

mkdir -p "$build_dir"
cp "$archive" "$build_dir/libandroid-virtual-ref-base-ptr-darwin.a"
cp "$manifest" "$build_dir/methods.tsv"
echo "virtual-ref-base-ptr: methods=2 owner=libandroid_runtime archive=Mach-O-arm64"
