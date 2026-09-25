#!/bin/bash
set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/.." && pwd)"
lock_file="$project_root/upstream/android16-hwui.lock"
output_dir="$project_root/_build/nativehelper-foundation"
object_dir="$output_dir/obj"

lock_value() {
  local key="$1"
  awk -F= -v key="$key" \
    '$1 == key { print substr($0, index($0, "=") + 1); exit }' "$lock_file"
}

libnativehelper_revision="$(lock_value LIBNATIVEHELPER_REVISION)"
frameworks_base_revision="$(lock_value FRAMEWORKS_BASE_REVISION)"
system_logging_revision="$(lock_value SYSTEM_LOGGING_REVISION)"

# The content hashes below bind the Android.bp module definitions and every
# source compiled by this gate to the immutable revisions above. Updating a
# revision therefore requires an intentional source-list and hash audit.
expected_libnativehelper_revision="5c36ece5ac42b20f0e384d9e57400f2f22aad9fb"
expected_frameworks_base_revision="99b01a65cc4c104933788b3143285ab6bae65827"
expected_system_logging_revision="d78b713380007d3c0dde14712cbcbec27f491ad9"

require_locked_revision() {
  local project="$1"
  local actual="$2"
  local expected="$3"
  if [[ -z "$actual" || "$actual" != "$expected" ]]; then
    echo "nativehelper-foundation: unsupported source lock for $project" >&2
    echo "  expected revision: $expected" >&2
    echo "  actual revision:   ${actual:-<missing>}" >&2
    exit 2
  fi
}

require_locked_revision platform/libnativehelper \
  "$libnativehelper_revision" "$expected_libnativehelper_revision"
require_locked_revision platform/frameworks/base \
  "$frameworks_base_revision" "$expected_frameworks_base_revision"
require_locked_revision platform/system/logging \
  "$system_logging_revision" "$expected_system_logging_revision"

verify_file() {
  local label="$1"
  local revision="$2"
  local subtree="$3"
  local file="$4"
  local expected="$5"
  if [[ ! -f "$file" ]]; then
    echo "nativehelper-foundation: required source is missing" >&2
    echo "  project/subtree: $label/$subtree" >&2
    echo "  revision:        $revision" >&2
    echo "  file:            $file" >&2
    echo "  expected sha256: $expected" >&2
    return 1
  fi
  local actual
  actual="$(shasum -a 256 "$file" | awk '{print $1}')"
  if [[ "$actual" != "$expected" ]]; then
    echo "nativehelper-foundation: source checksum mismatch" >&2
    echo "  project/subtree: $label/$subtree" >&2
    echo "  revision:        $revision" >&2
    echo "  file:            $file" >&2
    echo "  expected sha256: $expected" >&2
    echo "  actual sha256:   $actual" >&2
    return 1
  fi
}

verify_libnativehelper() {
  local root="$1"
  verify_file platform/libnativehelper "$libnativehelper_revision" . \
    "$root/Android.bp" eac19666c19e0b4263c697a7db0dc966ae1b377c9957a06b4d1950adc333ebcb
  verify_file platform/libnativehelper "$libnativehelper_revision" . \
    "$root/DlHelp.c" 549829df68089a241a1e1af40c6612faa280124434011c0c1449387e3d9fdfae
  verify_file platform/libnativehelper "$libnativehelper_revision" . \
    "$root/ExpandableString.c" 544dff4e24d875b4ede25db0d07a852d3c5a74944dfde88e6d15796004d626fd
  verify_file platform/libnativehelper "$libnativehelper_revision" . \
    "$root/JNIHelp.c" 097dd81bca400f813b21958ed6e5c83531f8b4d81231e698fb5933d74bed3a90
  verify_file platform/libnativehelper "$libnativehelper_revision" . \
    "$root/JniInvocation.c" 35ea3ecaab16f608698b9dd435f41bf69a46ef3465c2dffd37e4b1b6f69f7034
}

verify_nativehelper_jvm() {
  local root="$1"
  local subtree="libs/nativehelper_jvm"
  verify_file platform/frameworks/base "$frameworks_base_revision" "$subtree" \
    "$root/Android.bp" 9cfe177148733631b03d50a3a120fd840366c49f6a319fa0e3daa3f3660cb51b
  verify_file platform/frameworks/base "$frameworks_base_revision" "$subtree" \
    "$root/JNIPlatformHelp.c" d0390c7f7c61d982cade0a57710cb88e992fdfaa2e2988d18546719d01b29796
  verify_file platform/frameworks/base "$frameworks_base_revision" "$subtree" \
    "$root/JniConstants.c" c2af5792a1f82a22694353c82f28e3ccae556c61f3f9d9adb19727660a827e51
  verify_file platform/frameworks/base "$frameworks_base_revision" "$subtree" \
    "$root/JniConstants.h" 2687ec5351217fd7f393d97681067cc2ac665b410a69b1bdb73b80edbe54d0ac
  verify_file platform/frameworks/base "$frameworks_base_revision" "$subtree" \
    "$root/file_descriptor_jni.c" 5ab995671dee8397d07818133600e8e04891869243aef94d2d9915755937044e
}

verify_liblog_headers() {
  local root="$1"
  local subtree="liblog/include"
  verify_file platform/system/logging "$system_logging_revision" "$subtree" \
    "$root/android/log.h" 9139f3f005884dcfcba56476ab0df3794ba315177ac2aa915ad6932d1a7f24c6
  verify_file platform/system/logging "$system_logging_revision" "$subtree" \
    "$root/log/log.h" a88a797a352663075686bc6e3bf76675e1606367eaf12efd70f75706b84a595c
}

libnativehelper_source="$project_root/_aosp/libnativehelper-full"
nativehelper_jvm_source="$project_root/_aosp/frameworks/base/libs/nativehelper_jvm"
liblog_source="$project_root/_aosp/system/logging/liblog"
liblog_include_source="$liblog_source/include"

require_source_revision() {
  local source="$1"
  local expected="$2"
  if [[ ! -f "$source/.source-revision" ]] ||
      [[ "$(<"$source/.source-revision")" != "$expected" ]]; then
    echo "nativehelper-foundation: source revision mismatch: $source" >&2
    echo "run cargo run -p art-bootstrap -- sync" >&2
    exit 2
  fi
}

require_source_revision "$libnativehelper_source" "$libnativehelper_revision"
require_source_revision "$nativehelper_jvm_source" "$frameworks_base_revision"
require_source_revision "$liblog_source" "$system_logging_revision"
verify_libnativehelper "$libnativehelper_source"
verify_nativehelper_jvm "$nativehelper_jvm_source"
verify_liblog_headers "$liblog_include_source"

cc="$(command -v clang)"
libtool_bin="$(xcrun --find libtool)"

common_flags=(
  -std=c11
  # Soong global cflags: ALOGV/LOG_NDEBUG tracing and debug asserts are off.
  -DNDEBUG -UDEBUG
  -arch arm64
  -fPIC
  -fno-common
  -fvisibility=protected
  '-D__INTRODUCED_IN(n)='
  -I"$libnativehelper_source"
  -I"$libnativehelper_source/include"
  -I"$libnativehelper_source/include_jni"
  -I"$libnativehelper_source/include_platform"
  -I"$libnativehelper_source/include_platform_header_only"
  -I"$libnativehelper_source/header_only_include"
  -I"$liblog_include_source"
)

# These lists are the complete source lists from the two checksum-locked
# Android.bp modules. libnativehelper_jvm whole-archives libnativehelper_any_vm,
# so the final archive intentionally contains all seven objects.
any_vm_sources=(
  DlHelp.c
  ExpandableString.c
  JNIHelp.c
  JniInvocation.c
)
jvm_sources=(
  JNIPlatformHelp.c
  JniConstants.c
  file_descriptor_jni.c
)

any_vm_object_dir="$object_dir/libnativehelper_any_vm"
jvm_object_dir="$object_dir/libnativehelper_jvm"
mkdir -p "$any_vm_object_dir" "$jvm_object_dir"

any_vm_objects=()
for source in "${any_vm_sources[@]}"; do
  object="$any_vm_object_dir/${source%.c}.o"
  echo "nativehelper-foundation: compile libnativehelper_any_vm/$source"
  "$cc" "${common_flags[@]}" -c "$libnativehelper_source/$source" -o "$object"
  any_vm_objects+=("$object")
done

jvm_objects=()
for source in "${jvm_sources[@]}"; do
  object="$jvm_object_dir/${source%.c}.o"
  echo "nativehelper-foundation: compile libnativehelper_jvm/$source"
  "$cc" "${common_flags[@]}" -I"$nativehelper_jvm_source" \
    -c "$nativehelper_jvm_source/$source" -o "$object"
  jvm_objects+=("$object")
done

any_vm_archive="$output_dir/libnativehelper_any_vm.a"
jvm_archive="$output_dir/libnativehelper_jvm.a"
"$libtool_bin" -static -o "$any_vm_archive" "${any_vm_objects[@]}"
"$libtool_bin" -static -o "$jvm_archive" \
  "${any_vm_objects[@]}" "${jvm_objects[@]}"

for object in "${any_vm_objects[@]}" "${jvm_objects[@]}"; do
  file "$object" | grep -F 'Mach-O 64-bit object arm64' >/dev/null
done
for archive in "$any_vm_archive" "$jvm_archive"; do
  file "$archive" | grep -F 'current ar archive' >/dev/null
  lipo -info "$archive" | grep -F 'architecture: arm64' >/dev/null
done

verify_archive_members() {
  local archive="$1"
  shift
  local expected=("$@")
  local actual
  actual="$(ar -t "$archive" | grep -v '^__.SYMDEF' | sort)"
  local wanted
  wanted="$(printf '%s\n' "${expected[@]}" | sort)"
  if [[ "$actual" != "$wanted" ]]; then
    echo "nativehelper-foundation: archive member mismatch: $archive" >&2
    diff -u <(printf '%s\n' "$wanted") <(printf '%s\n' "$actual") >&2 || true
    exit 1
  fi
}

verify_archive_members "$any_vm_archive" \
  DlHelp.o ExpandableString.o JNIHelp.o JniInvocation.o
verify_archive_members "$jvm_archive" \
  DlHelp.o ExpandableString.o JNIHelp.o JniInvocation.o \
  JNIPlatformHelp.o JniConstants.o file_descriptor_jni.o

require_definition() {
  local archive="$1"
  local symbol="$2"
  if ! nm -gU "$archive" | grep -E "[[:space:]]T _$symbol$" >/dev/null; then
    echo "nativehelper-foundation: missing expected definition _$symbol in $archive" >&2
    exit 1
  fi
}

for symbol in \
  DlOpenLibrary ExpandableStringInitialize JniInvocationCreate \
  JNI_CreateJavaVM jniRegisterNativeMethods jniThrowException \
  jniThrowNullPointerException jniThrowRuntimeException; do
  require_definition "$any_vm_archive" "$symbol"
  require_definition "$jvm_archive" "$symbol"
done
for symbol in \
  jniGetNioBufferBaseArray jniGetNioBufferBaseArrayOffset \
  jniGetNioBufferPointer jniGetNioBufferFields jniUninitializeConstants \
  AFileDescriptor_create AFileDescriptor_getFd AFileDescriptor_setFd; do
  require_definition "$jvm_archive" "$symbol"
done

echo "nativehelper-foundation: libnativehelper_any_vm objects=${#any_vm_objects[@]}"
echo "nativehelper-foundation: libnativehelper_jvm objects=$((${#any_vm_objects[@]} + ${#jvm_objects[@]}))"
echo "nativehelper-foundation: archives=$output_dir"
