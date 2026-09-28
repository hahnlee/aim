#!/bin/bash
set -euo pipefail
export LC_ALL=C

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/.." && pwd)"
lock_file="$project_root/upstream/android16-file-descriptor-darwin.lock"
source_root="$project_root/_aosp/libcore-file-descriptor"
native_root="$source_root/ojluni/src/main/native"
build_dir="$project_root/_build/file-descriptor-darwin"
patch_file="$project_root/patches/libcore-openjdk/0002-darwin-file-descriptor-syscalls.patch"

# shellcheck disable=SC1090
source "$lock_file"

sha256() { shasum -a 256 "$1" | awk '{print $1}'; }
fail() { echo "file-descriptor: $*" >&2; exit 3; }

materialize() {
  local relative="$1" expected="$2" destination="$source_root/$1"
  if [[ -f "$destination" ]]; then
    [[ "$(sha256 "$destination")" == "$expected" ]] ||
      fail "checksum mismatch: $destination"
    return
  fi
  mkdir -p "$(dirname "$destination")"
  local staged
  staged="$(mktemp "${destination}.download.XXXXXX")"
  curl -fsSL \
    "https://android.googlesource.com/$LIBCORE_PROJECT/+/$LIBCORE_REVISION/$relative?format=TEXT" \
    | base64 -D > "$staged"
  [[ "$(sha256 "$staged")" == "$expected" ]] ||
    fail "download checksum mismatch: $relative"
  mv "$staged" "$destination"
}

materialize ojluni/src/main/native/Android.bp "$NATIVE_ANDROID_BP_SHA256"
materialize ojluni/src/main/native/OnLoad.cpp "$ONLOAD_CPP_SHA256"
materialize ojluni/src/main/native/FileDescriptor_md.c "$FILE_DESCRIPTOR_MD_C_SHA256"
materialize ojluni/src/main/native/jni_util.c "$JNI_UTIL_C_SHA256"
materialize ojluni/src/main/native/jni_util_md.c "$JNI_UTIL_MD_C_SHA256"
materialize ojluni/src/main/native/jni_util.h "$JNI_UTIL_H_SHA256"
materialize ojluni/src/main/native/jlong.h "$JLONG_H_SHA256"
materialize ojluni/src/main/native/jlong_md.h "$JLONG_MD_H_SHA256"
materialize ojluni/src/main/native/jvm.h "$JVM_H_SHA256"
materialize ojluni/src/main/native/jvm_md.h "$JVM_MD_H_SHA256"
materialize ojluni/src/main/native/classfile_constants.h \
  "$CLASSFILE_CONSTANTS_H_SHA256"

[[ "$(sha256 "$patch_file")" == "$DARWIN_PATCH_SHA256" ]] ||
  fail "Darwin syscall patch checksum mismatch"

stage="$(mktemp -d "${TMPDIR:-/tmp}/aim-file-descriptor.XXXXXX")"
trap 'rm -rf "$stage"' EXIT
patched_root="$stage/source"
mkdir -p "$patched_root/ojluni/src/main/native"
cp "$native_root/FileDescriptor_md.c" \
  "$patched_root/ojluni/src/main/native/FileDescriptor_md.c"
patch --batch --forward -p1 -d "$patched_root" < "$patch_file" >/dev/null
patched_source="$patched_root/ojluni/src/main/native/FileDescriptor_md.c"
[[ "$(sha256 "$patched_source")" == "$PATCHED_FILE_DESCRIPTOR_MD_C_SHA256" ]] ||
  fail "patched FileDescriptor source checksum mismatch"
grep -F 'GetFieldID(env, fdClass, "descriptor", "I")' "$patched_source" >/dev/null ||
  fail "Android device FileDescriptor.descriptor:I layout drift"
grep -F 'SO_TYPE' "$patched_source" >/dev/null ||
  fail "Darwin socket predicate patch missing"

python3 - "$native_root/Android.bp" <<'PY'
import re
import sys
from pathlib import Path

text = Path(sys.argv[1]).read_text()
start = text.index('name: "libopenjdk_native_srcs"')
body_start = text.index('srcs: [', start)
body_end = text.index('],', body_start)
sources = re.findall(r'"([^" ]+\.(?:c|cpp))"', text[body_start:body_end])
if len(sources) != 59 or 'FileDescriptor_md.c' not in sources:
    raise SystemExit(f'libopenjdk source membership drift: count={len(sources)}')
PY

methods="$stage/file-descriptor-methods.tsv"
python3 - "$native_root/FileDescriptor_md.c" > "$methods" <<'PY'
import re
import sys
from pathlib import Path

text = Path(sys.argv[1]).read_text()
start = text.index('static JNINativeMethod gMethods[]')
body = text[start:text.index('void register_java_io_FileDescriptor', start)]
for kind, name, signature in re.findall(
        r'(NATIVE_METHOD|CRITICAL_NATIVE_METHOD)\(FileDescriptor,\s*([^,]+),\s*"([^"]+)"\)',
        body):
    print(f'{kind}\t{name}\t{signature}')
PY
method_count="$(wc -l < "$methods" | tr -d ' ')"
method_sha="$(sha256 "$methods")"
[[ "$method_count" == "$FILE_DESCRIPTOR_METHOD_COUNT" &&
   "$method_sha" == "$FILE_DESCRIPTOR_METHOD_MANIFEST_SHA256" ]] ||
  fail "method table drift count=$method_count sha=$method_sha"
grep -Fx $'NATIVE_METHOD\tsync\t()V' "$methods" >/dev/null ||
  fail "sync native missing"
grep -Fx $'CRITICAL_NATIVE_METHOD\tisSocket\t(I)Z' "$methods" >/dev/null ||
  fail "critical isSocket native missing"
grep -Fx $'CRITICAL_NATIVE_METHOD\tgetAppend\t(I)Z' "$methods" >/dev/null ||
  fail "critical getAppend native missing"
grep -F 'register_java_io_FileDescriptor(env);' "$native_root/OnLoad.cpp" >/dev/null ||
  fail "canonical OnLoad registrar call missing"

nativehelper_source="$project_root/_aosp/libnativehelper-full"
device_nativehelper="$project_root/_build/nativehelper-device-foundation/libnativehelper-device-darwin.a"
file_input_stream="$project_root/_build/file-input-stream-darwin/libopenjdk-file-input-stream-darwin.a"
openjdkjvm="$project_root/_build/openjdkjvm-darwin/libopenjdkjvm-darwin.a"
liblog_include="$project_root/_aosp/system/logging/liblog/include"
liblog="$project_root/_build/graphics-foundations/liblog-darwin.a"
for required in \
  "$device_nativehelper" "$file_input_stream" "$openjdkjvm" "$liblog" \
  "$nativehelper_source/include_jni/jni.h" \
  "$nativehelper_source/include/android/file_descriptor_jni.h" \
  "$nativehelper_source/include/nativehelper/JNIHelp.h" \
  "$nativehelper_source/include_platform_header_only/nativehelper/jni_macros.h" \
  "$liblog_include/android/log.h" \
  "$project_root/probes/android16_file_descriptor_jni.c" \
  "$project_root/probes/file-descriptor/FileDescriptorDarwinSmoke.java"; do
  [[ -e "$required" ]] || fail "missing build dependency: $required"
done

cc="$(xcrun --find clang)"
cxx="$(xcrun --find clang++)"
libtool_bin="$(xcrun --find libtool)"
sdk_root="$(xcrun --sdk macosx --show-sdk-path)"
objects="$stage/objects"
mkdir -p "$objects"
common_flags=(
  -std=gnu11 -arch arm64 -isysroot "$sdk_root" -fPIC
  -ffunction-sections -fdata-sections -DMACOSX -D_ALLBSD_SOURCE
  -Wall -Wextra -Werror -Wno-unused-parameter -Wno-unused-variable
  -Wno-deprecated-declarations
  -I"$native_root"
  -I"$nativehelper_source/include_jni"
  -I"$nativehelper_source/include"
  -I"$nativehelper_source/include_platform"
  -I"$nativehelper_source/include_platform_header_only"
  -I"$nativehelper_source/header_only_include"
  -I"$liblog_include"
)

source_object="$objects/FileDescriptor_md.o"
"$cc" "${common_flags[@]}" -c "$patched_source" -o "$source_object"
[[ "$(file "$source_object")" == *"Mach-O 64-bit object arm64"* ]] ||
  fail "FileDescriptor object is not Darwin arm64"
archive="$stage/libopenjdk-file-descriptor-darwin.a"
"$libtool_bin" -static -o "$archive" "$source_object"
[[ "$(lipo -archs "$archive")" == arm64 ]] || fail "archive is not arm64"
member_count="$({ ar -t "$archive" || true; } | grep -v '^__\.SYMDEF' | wc -l | tr -d ' ')"
[[ "$member_count" == "$FILE_DESCRIPTOR_SOURCE_COUNT" ]] ||
  fail "archive member count=$member_count"

symbols="$stage/archive-symbols.txt"
nm -aC "$archive" > "$symbols"
for definition in \
  _register_java_io_FileDescriptor _FileDescriptor_sync \
  _FileDescriptor_isSocket _FileDescriptor_getAppend _IO_fd_fdID; do
  grep -E " [TDSBC] ${definition}$" "$symbols" >/dev/null ||
    fail "complete definition missing: $definition"
done

device_probe="$objects/device-contract.o"
"$cc" "${common_flags[@]}" -DAIM_FILE_DESCRIPTOR_DEVICE_CONTRACT \
  -c "$project_root/probes/android16_file_descriptor_jni.c" -o "$device_probe"
device_library="$stage/libfile-descriptor-device-closure.dylib"
"$cxx" -arch arm64 -isysroot "$sdk_root" -dynamiclib \
  "$device_probe" -Wl,-force_load,"$archive" \
  "$file_input_stream" "$openjdkjvm" "$device_nativehelper" "$liblog" \
  -Wl,-undefined,dynamic_lookup \
  -Wl,-exported_symbol,_register_java_io_FileDescriptor \
  -Wl,-exported_symbol,_aim_file_descriptor_device_contract \
  -Wl,-dead_strip -framework CoreFoundation -o "$device_library"
device_symbols="$stage/device-closure-symbols.txt"
nm -aC "$device_library" > "$device_symbols"
for retained in \
  _register_java_io_FileDescriptor _AFileDescriptor_getFd \
  _AFileDescriptor_setFd _JniConstants_FileDescriptor_descriptor; do
  grep -E " [TtDdSsBbCc] ${retained}$" "$device_symbols" >/dev/null ||
    fail "device closure did not retain $retained"
done
if grep -E ' [TtDdSsBbCc] _JniConstants_FileDescriptor_fd$' "$device_symbols" >/dev/null; then
  fail "device closure selected host FileDescriptor.fd:I nativehelper"
fi
device_undefined="$stage/device-retained-undefined.txt"
nm -u "$device_library" | sed 's/^[[:space:]]*//' | sort -u > "$device_undefined"
for closed in _JVM_Sync _JNU_ThrowByName _jniRegisterNativeMethods \
  _AFileDescriptor_getFd _AFileDescriptor_setFd; do
  if grep -Fx "$closed" "$device_undefined" >/dev/null; then
    fail "device provider closure retained undefined $closed"
  fi
done

managed_probe="$objects/managed-probe.o"
"$cc" "${common_flags[@]}" \
  -c "$project_root/probes/android16_file_descriptor_jni.c" -o "$managed_probe"
managed_library="$stage/libfile-descriptor-managed.dylib"
"$cxx" -arch arm64 -isysroot "$sdk_root" -dynamiclib \
  "$managed_probe" "$archive" "$file_input_stream" "$openjdkjvm" \
  -Wl,-undefined,dynamic_lookup \
  -Wl,-exported_symbol,_JNI_OnLoad -Wl,-dead_strip -o "$managed_library"
classes="$stage/classes"
mkdir -p "$classes"
javac --release 17 -encoding UTF-8 -d "$classes" \
  "$project_root/probes/file-descriptor/FileDescriptorDarwinSmoke.java"
managed_output="$(java -cp "$classes" \
  dev.aim.probe.FileDescriptorDarwinSmoke "$managed_library")"
expected='managed-file-descriptor: methods=3 sync=pass socket=pass append=pass'
[[ "$managed_output" == "$expected" ]] ||
  fail "managed acceptance failed: $managed_output"

duplicates="$stage/duplicate-full-owners.txt"
: > "$duplicates"
for provider in \
  "$project_root/_build/runtime-bootstrap/libart-runtime-bootstrap-darwin.a" \
  "$project_root/_build/runtime-graphics-bootstrap/libart-runtime-graphics-bootstrap-darwin.a" \
  "$project_root/_build/libcore-darwin-linux/libcore-darwin-linux.a"; do
  [[ -f "$provider" ]] || continue
  if nm -aC "$provider" | grep -E \
    ' [TDSBC] _(register_java_io_FileDescriptor|FileDescriptor_sync|FileDescriptor_isSocket|FileDescriptor_getAppend|IO_fd_fdID)$' \
    >> "$duplicates"; then
    fail "duplicate complete FileDescriptor owner atom in $provider"
  fi
done

partial_contract="$stage/atomic-replacement.txt"
cat > "$partial_contract" <<'EOF'
integration-requires=remove compat file_descriptor_methods[2]
integration-requires=remove direct Register(java/io/FileDescriptor,...,2)
integration-requires=call register_java_io_FileDescriptor before IOUtil
integration-forbids=mixing partial getAppend/isSocket with complete registrar
link-order=consumer,libopenjdk-file-descriptor-darwin.a,libopenjdk-file-input-stream-darwin.a,libopenjdkjvm-darwin.a,libnativehelper-device-darwin.a,liblog-darwin.a
EOF

undefined="$stage/archive-undefined.txt"
nm -u "$archive" | sed 's/^[[:space:]]*//' | sort -u > "$undefined"
mkdir -p "$build_dir"
cp "$archive" "$build_dir/libopenjdk-file-descriptor-darwin.a"
cp "$methods" "$build_dir/file-descriptor-methods.tsv"
cp "$symbols" "$build_dir/archive-symbols.txt"
cp "$undefined" "$build_dir/archive-undefined.txt"
cp "$device_undefined" "$build_dir/device-retained-undefined.txt"
cp "$device_symbols" "$build_dir/device-closure-symbols.txt"
cp "$duplicates" "$build_dir/duplicate-full-owners.txt"
cp "$partial_contract" "$build_dir/atomic-replacement.txt"
cp "$patched_source" "$build_dir/FileDescriptor_md.darwin.c"

echo "file-descriptor: sources=1/59 methods=3 layout=descriptor:I device-nativehelper=pass managed=sync-socket-append-pass duplicate-full-owners=0 archive=Mach-O-arm64"
