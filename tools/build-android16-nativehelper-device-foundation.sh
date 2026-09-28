#!/bin/bash
set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/.." && pwd)"
lock_file="$project_root/upstream/android16-nativehelper-device.lock"
source_root="$project_root/_aosp/libnativehelper-full"
host_root="$project_root/_aosp/frameworks/base/libs/nativehelper_jvm"
build_dir="$project_root/_build/nativehelper-device-foundation"

# shellcheck disable=SC1090
source "$lock_file"

sha256() { shasum -a 256 "$1" | awk '{print $1}'; }
fail() {
  echo "nativehelper-device: $*" >&2
  exit 3
}
verify_hash() {
  local path="$1" expected="$2"
  [[ -f "$path" ]] || fail "missing locked source: $path"
  local actual
  actual="$(sha256 "$path")"
  [[ "$actual" == "$expected" ]] ||
    fail "checksum mismatch: $path expected=$expected actual=$actual"
}

[[ -f "$source_root/.source-revision" ]] || fail "missing device source revision"
[[ "$(<"$source_root/.source-revision")" == "$LIBNATIVEHELPER_REVISION" ]] ||
  fail "device source revision mismatch"
[[ -f "$host_root/.source-revision" ]] || fail "missing host source revision"
[[ "$(<"$host_root/.source-revision")" == "$FRAMEWORKS_BASE_REVISION" ]] ||
  fail "host source revision mismatch"

verify_hash "$source_root/Android.bp" "$ANDROID_BP_SHA256"
verify_hash "$source_root/DlHelp.c" "$DL_HELP_SHA256"
verify_hash "$source_root/ExpandableString.c" "$EXPANDABLE_STRING_SHA256"
verify_hash "$source_root/JNIHelp.c" "$JNI_HELP_SHA256"
verify_hash "$source_root/JniInvocation.c" "$JNI_INVOCATION_SHA256"
verify_hash "$source_root/JNIPlatformHelp.c" "$JNI_PLATFORM_HELP_SHA256"
verify_hash "$source_root/JniConstants.c" "$JNI_CONSTANTS_SHA256"
verify_hash "$source_root/JniConstants.h" "$JNI_CONSTANTS_H_SHA256"
verify_hash "$source_root/file_descriptor_jni.c" "$FILE_DESCRIPTOR_JNI_SHA256"
verify_hash "$host_root/Android.bp" "$HOST_ANDROID_BP_SHA256"
verify_hash "$host_root/JniConstants.c" "$HOST_JNI_CONSTANTS_SHA256"
verify_hash "$host_root/file_descriptor_jni.c" "$HOST_FILE_DESCRIPTOR_JNI_SHA256"

stage_parent="$build_dir/stage"
mkdir -p "$stage_parent"
stage="$(mktemp -d "$stage_parent/build.XXXXXX")"
trap 'rm -rf "$stage"' EXIT

sources="$stage/device-sources.txt"
python3 - "$source_root/Android.bp" > "$sources" <<'PY'
import re
import sys
from pathlib import Path

text = Path(sys.argv[1]).read_text()
for module in ('libnativehelper_any_vm', 'libnativehelper'):
    start = text.index(f'name: "{module}"')
    src_start = text.index('srcs: [', start) + len('srcs: [')
    src_end = text.index('],', src_start)
    for source in re.findall(r'"([^" ]+\.c)"', text[src_start:src_end]):
        print(source)
PY
source_count="$(wc -l < "$sources" | tr -d ' ')"
source_sha="$(sha256 "$sources")"
[[ "$source_count" == "$DEVICE_SOURCE_COUNT" &&
   "$source_sha" == "$DEVICE_SOURCE_LIST_SHA256" ]] ||
  fail "Android.bp device module source drift count=$source_count sha=$source_sha"

# Prove the two pinned module variants differ at the managed FileDescriptor ABI,
# not merely by archive name.
grep -F 'V(FileDescriptor, descriptor, "I", false)' \
  "$source_root/JniConstants.c" >/dev/null || fail "device descriptor:I field missing"
grep -F 'JniConstants_FileDescriptor_descriptor(env)' \
  "$source_root/file_descriptor_jni.c" >/dev/null || fail "device descriptor accessor missing"
grep -F 'JniConstants_FileDescriptor_setInt$(env)' \
  "$source_root/file_descriptor_jni.c" >/dev/null || fail "device setInt$ setter missing"
grep -F 'V(FileDescriptor, fd, "I", false)' \
  "$host_root/JniConstants.c" >/dev/null || fail "host fd:I field missing"
grep -F 'JniConstants_FileDescriptor_fd(env)' \
  "$host_root/file_descriptor_jni.c" >/dev/null || fail "host fd accessor missing"

cc="$(command -v clang)"
cxx="$(command -v clang++)"
libtool_bin="$(xcrun --find libtool)"
objects="$stage/objects"
mkdir -p "$objects"
flags=(
  -std=c11 -arch arm64 -O2 -fPIC -fno-common -fvisibility=protected
  '-D__INTRODUCED_IN(n)='
  -Wall -Werror -Wextra -Wno-unused-parameter -Wno-unused-function
  -I"$source_root" -I"$source_root/include" -I"$source_root/include_jni"
  -I"$source_root/include_platform" -I"$source_root/include_platform_header_only"
  -I"$source_root/header_only_include"
  -I"$project_root/_aosp/system/logging/liblog/include"
)

compiled=()
while IFS= read -r source; do
  object="$objects/${source%.c}.o"
  echo "nativehelper-device: compile $source"
  "$cc" "${flags[@]}" -c "$source_root/$source" -o "$object"
  [[ "$(file "$object")" == *"Mach-O 64-bit object arm64"* ]] ||
    fail "non-arm64 object: $object"
  compiled+=("$object")
done < "$sources"

archive="$stage/libnativehelper-device-darwin.a"
"$libtool_bin" -static -o "$archive" "${compiled[@]}"
members="$stage/archive-members.txt"
ar -t "$archive" | grep -v '^__.SYMDEF' > "$members"
[[ "$(wc -l < "$members" | tr -d ' ')" == "$DEVICE_SOURCE_COUNT" ]] ||
  fail "device archive member count mismatch"
[[ "$(file "$archive")" == *"ar archive"* && "$(lipo -archs "$archive")" == arm64 ]] ||
  fail "device archive is not arm64"

definitions="$stage/device-definitions.txt"
nm -gU "$archive" | sort -u > "$definitions"
for symbol in \
  _JniConstants_FileDescriptor_descriptor \
  '_JniConstants_FileDescriptor_setInt$' \
  _AFileDescriptor_getFd _AFileDescriptor_setFd \
  _jniRegisterNativeMethods _jniThrowException; do
  grep -F " T $symbol" "$definitions" >/dev/null ||
    fail "device definition missing: $symbol"
done
if grep -E '[[:space:]]T _JniConstants_FileDescriptor_fd$' "$definitions" >/dev/null; then
  fail "device archive leaked host FileDescriptor.fd accessor"
fi

"$script_dir/build-android16-nativehelper-foundation.sh" >/dev/null
host_archive="$project_root/_build/nativehelper-foundation/libnativehelper_jvm.a"
host_definitions="$stage/host-definitions.txt"
nm -gU "$host_archive" | sort -u > "$host_definitions"
grep -E '[[:space:]]T _JniConstants_FileDescriptor_fd$' \
  "$host_definitions" >/dev/null || fail "host archive fd accessor missing"
if grep -E '[[:space:]]T _JniConstants_FileDescriptor_descriptor$' \
  "$host_definitions" >/dev/null; then
  fail "host archive leaked device descriptor accessor"
fi

probe_object="$objects/nativehelper-device-layout-smoke.o"
"$cc" -std=c11 -arch arm64 -Wall -Werror \
  -I"$source_root/include" -I"$source_root/include_jni" \
  "$project_root/probes/android16_nativehelper_device_layout_smoke.c" \
  -c -o "$probe_object"
probe="$stage/nativehelper-device-layout-smoke"
"$cxx" -arch arm64 "$probe_object" \
  "$archive" "$project_root/_build/graphics-foundations/liblog-darwin.a" \
  -o "$probe"
"$probe" > "$stage/layout-smoke.log"

abi_manifest="$stage/file-descriptor-layout.txt"
printf '%s\n' \
  "device-project=$LIBNATIVEHELPER_PROJECT" \
  "device-revision=$LIBNATIVEHELPER_REVISION" \
  'device-field=java.io.FileDescriptor.descriptor:I' \
  'device-get=_JniConstants_FileDescriptor_descriptor' \
  'device-set=java.io.FileDescriptor.setInt$(I)V' \
  "host-project=$FRAMEWORKS_BASE_PROJECT" \
  "host-revision=$FRAMEWORKS_BASE_REVISION" \
  'host-field=java.io.FileDescriptor.fd:I' \
  'host-get=_JniConstants_FileDescriptor_fd' \
  'host-set=JNIEnv.SetIntField' > "$abi_manifest"

mkdir -p "$build_dir"
cp "$archive" "$build_dir/libnativehelper-device-darwin.a"
cp "$sources" "$build_dir/device-sources.txt"
cp "$members" "$build_dir/archive-members.txt"
cp "$definitions" "$build_dir/device-definitions.txt"
cp "$host_definitions" "$build_dir/host-definitions.txt"
cp "$abi_manifest" "$build_dir/file-descriptor-layout.txt"
cp "$stage/layout-smoke.log" "$build_dir/layout-smoke.log"

echo "nativehelper-device: sources=$source_count archive=arm64 device-field=descriptor:I host-field=fd:I smoke=pass"
