#!/bin/bash
set -euo pipefail
export LC_ALL=C

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/.." && pwd)"
lock_file="$project_root/upstream/android16-file-input-stream-darwin.lock"
source_root="$project_root/_aosp/libcore-file-input-stream"
native_root="$source_root/ojluni/src/main/native"
build_dir="$project_root/_build/file-input-stream-darwin"
patch_file="$project_root/patches/libcore-openjdk/0001-darwin-nativehelper-file-descriptor.patch"
descriptor_patch="$project_root/patches/libcore-openjdk/0002-darwin-file-input-stream-virtual-descriptors.patch"

# shellcheck disable=SC1090
source "$lock_file"

sha256() { shasum -a 256 "$1" | awk '{print $1}'; }
fail() { echo "file-input-stream: $*" >&2; exit 3; }

materialize() {
  local relative="$1" expected="$2"
  local destination="$source_root/$relative"
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
materialize ojluni/src/main/native/FileInputStream.c "$FILE_INPUT_STREAM_C_SHA256"
materialize ojluni/src/main/native/io_util_md.c "$IO_UTIL_MD_C_SHA256"
materialize ojluni/src/main/native/jni_util.c "$JNI_UTIL_C_SHA256"
materialize ojluni/src/main/native/jni_util_md.c "$JNI_UTIL_MD_C_SHA256"
materialize ojluni/src/main/native/jni_util.h "$JNI_UTIL_H_SHA256"
materialize ojluni/src/main/native/jlong.h "$JLONG_H_SHA256"
materialize ojluni/src/main/native/jlong_md.h "$JLONG_MD_H_SHA256"
materialize ojluni/src/main/native/jvm.h "$JVM_H_SHA256"
materialize ojluni/src/main/native/jvm_md.h "$JVM_MD_H_SHA256"
materialize ojluni/src/main/native/classfile_constants.h \
  "$CLASSFILE_CONSTANTS_H_SHA256"
materialize ojluni/src/main/native/io_util.h "$IO_UTIL_H_SHA256"
materialize ojluni/src/main/native/io_util_md.h "$IO_UTIL_MD_H_SHA256"

stage="$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-fis.XXXXXX")"
trap 'rm -rf "$stage"' EXIT
patched_root="$stage/patched"
mkdir -p "$patched_root/ojluni/src/main/native"
cp "$native_root/io_util_md.c" "$patched_root/ojluni/src/main/native/io_util_md.c"
cp "$native_root/FileInputStream.c" \
  "$patched_root/ojluni/src/main/native/FileInputStream.c"
patch --batch --forward -p1 -d "$patched_root" < "$patch_file" >/dev/null
patch --batch --forward -p1 -d "$patched_root" < "$descriptor_patch" >/dev/null
host_patched_root="$stage/host-patched"
mkdir -p "$host_patched_root/ojluni/src/main/native"
cp "$native_root/io_util_md.c" \
  "$host_patched_root/ojluni/src/main/native/io_util_md.c"
patch --batch --forward -p1 -d "$host_patched_root" < "$patch_file" >/dev/null
[[ "$(sha256 "$patch_file")" == "$NATIVEHELPER_FD_PATCH_SHA256" ]] ||
  fail "nativehelper FileDescriptor patch checksum mismatch"
[[ "$(sha256 "$descriptor_patch")" == "$VIRTUAL_DESCRIPTOR_PATCH_SHA256" ]] ||
  fail "virtual descriptor patch checksum mismatch"
[[ "$(sha256 "$patched_root/ojluni/src/main/native/io_util_md.c")" == \
   "$PATCHED_IO_UTIL_MD_C_SHA256" ]] || fail "patched io_util_md.c checksum mismatch"
[[ "$(sha256 "$patched_root/ojluni/src/main/native/FileInputStream.c")" == \
   "$PATCHED_FILE_INPUT_STREAM_C_SHA256" ]] || \
  fail "patched FileInputStream.c checksum mismatch"
grep -F 'AFileDescriptor_getFd(env, fdo)' \
  "$patched_root/ojluni/src/main/native/io_util_md.c" >/dev/null ||
  fail "nativehelper FileDescriptor patch not applied"
grep -F '#define fstat64 darwin_art_openjdk_nio_fstat' \
  "$patched_root/ojluni/src/main/native/io_util_md.c" >/dev/null ||
  fail "virtual descriptor patch not applied"
grep -F '#define ioctl darwin_art_openjdk_nio_ioctl' \
  "$patched_root/ojluni/src/main/native/FileInputStream.c" >/dev/null ||
  fail "FileInputStream ioctl redirect patch not applied"

source_manifest="$stage/libopenjdk-sources.txt"
python3 - "$native_root/Android.bp" > "$source_manifest" <<'PY'
import re
import sys
from pathlib import Path

text = Path(sys.argv[1]).read_text()
start = text.index('name: "libopenjdk_native_srcs"')
body_start = text.index('srcs: [', start) + len('srcs: [')
body_end = text.index('],', body_start)
for source in re.findall(r'"([^" ]+\.(?:c|cpp))"', text[body_start:body_end]):
    print(source)
PY
source_count="$(wc -l < "$source_manifest" | tr -d ' ')"
[[ "$source_count" == "$LIBOPENJDK_SOURCE_COUNT" ]] ||
  fail "libopenjdk source count=$source_count"

closure_manifest="$stage/file-input-stream-sources.txt"
python3 - "$source_manifest" > "$closure_manifest" <<'PY'
import sys
from pathlib import Path

sources = set(Path(sys.argv[1]).read_text().splitlines())
closure = ["FileInputStream.c", "io_util_md.c", "jni_util.c", "jni_util_md.c"]
missing = [name for name in closure if name not in sources]
if missing:
    raise SystemExit(f"FileInputStream closure left Android.bp: {missing}")
print(*closure, sep="\n")
PY
closure_count="$(wc -l < "$closure_manifest" | tr -d ' ')"
closure_sha="$(sha256 "$closure_manifest")"
[[ "$closure_count" == "$FILE_INPUT_STREAM_SOURCE_COUNT" &&
   "$closure_sha" == "$FILE_INPUT_STREAM_SOURCE_MANIFEST_SHA256" ]] ||
  fail "closure drift count=$closure_count sha=$closure_sha"

method_manifest="$stage/file-input-stream-methods.tsv"
python3 - "$native_root/FileInputStream.c" > "$method_manifest" <<'PY'
import re
import sys
from pathlib import Path

text = Path(sys.argv[1]).read_text()
start = text.index("static JNINativeMethod gMethods[]")
end = text.index("void register_java_io_FileInputStream", start)
methods = re.findall(
    r'NATIVE_METHOD\(Java_java_io_FileInputStream,\s*([A-Za-z0-9_]+),\s*"([^"]+)"\)',
    text[start:end],
)
for name, signature in methods:
    print(f"{name}\t{signature}")
PY
method_count="$(wc -l < "$method_manifest" | tr -d ' ')"
method_sha="$(sha256 "$method_manifest")"
[[ "$method_count" == "$FILE_INPUT_STREAM_METHOD_COUNT" &&
   "$method_sha" == "$FILE_INPUT_STREAM_METHOD_MANIFEST_SHA256" ]] ||
  fail "method table drift count=$method_count sha=$method_sha"
grep -F 'register_java_io_FileInputStream(env);' "$native_root/OnLoad.cpp" >/dev/null ||
  fail "canonical OnLoad registrar call missing"

device_nativehelper="$project_root/_build/nativehelper-device-foundation/libnativehelper-device-darwin.a"
host_nativehelper="$project_root/_build/nativehelper-foundation/libnativehelper_jvm.a"
nativehelper_source="$project_root/_aosp/libnativehelper-full"
openjdkjvm="$project_root/_build/openjdkjvm-darwin/libopenjdkjvm-darwin.a"
liblog_include="$project_root/_aosp/system/logging/liblog/include"
liblog="$project_root/_build/graphics-foundations/liblog-darwin.a"
for required in \
  "$patch_file" \
  "$descriptor_patch" \
  "$device_nativehelper" \
  "$host_nativehelper" \
  "$openjdkjvm" \
  "$nativehelper_source/include/android/file_descriptor_jni.h" \
  "$nativehelper_source/include/nativehelper/JNIHelp.h" \
  "$liblog_include/android/log.h" \
  "$liblog" \
  "$project_root/probes/android16_file_input_stream_jni.c" \
  "$project_root/probes/file-input-stream/FileInputStreamDarwinSmoke.java"; do
  [[ -e "$required" ]] || fail "missing build dependency: $required"
done

cc="$(xcrun --find clang)"
libtool_bin="$(xcrun --find libtool)"
sdk_root="$(xcrun --sdk macosx --show-sdk-path)"
objects="$stage/objects"
mkdir -p "$objects"
common_flags=(
  -std=gnu11 -arch arm64 -isysroot "$sdk_root" -fPIC
  -ffunction-sections -fdata-sections
      -DMACOSX -D_ALLBSD_SOURCE -DDARWIN_ART_NATIVEHELPER_FILE_DESCRIPTOR
      -DDARWIN_ART_OPENJDK_VIRTUAL_DESCRIPTORS
  -Wall -Wextra -Werror
  -Wno-unused-parameter -Wno-unused-variable -Wno-sign-compare
  -Wno-deprecated-declarations -Wno-incompatible-pointer-types-discards-qualifiers
  -I"$native_root"
  -I"$nativehelper_source/include_jni"
  -I"$nativehelper_source/include"
  -I"$nativehelper_source/include_platform"
  -I"$nativehelper_source/include_platform_header_only"
  -I"$nativehelper_source/header_only_include"
  -I"$liblog_include"
)

while IFS= read -r source; do
  input="$native_root/$source"
  if [[ "$source" == io_util_md.c ]]; then
    input="$patched_root/ojluni/src/main/native/io_util_md.c"
  elif [[ "$source" == FileInputStream.c ]]; then
    input="$patched_root/ojluni/src/main/native/FileInputStream.c"
  fi
  object="$objects/${source%.*}.o"
  if [[ "$source" == FileInputStream.c || "$source" == io_util_md.c ]]; then
    # FileDescriptor.fd is an Android guest descriptor. FileInputStream's
    # direct skip0() path and io_util_md's shared read/available helpers both
    # perform descriptor syscalls, so route both translation units through the
    # production Bionic fd owner. The AOSP BSD header aliases fstat64/lseek64
    # to these redirected fstat/lseek functions.
    "$cc" "${common_flags[@]}" \
      -I"$project_root/tools/bionic-errno-tls/include" \
      -I"$project_root/tools/bionic-fs-facade/include" \
      -I"$project_root/tools/bionic-ioctl-facade/include" \
      -I"$project_root/tools/bionic-socket-broker-adapter/include" \
      -include "$project_root/compat/darwin_openjdk_nio_fs_redirect.h" \
      -c "$input" -o "$object"
  else
    "$cc" "${common_flags[@]}" -c "$input" -o "$object"
  fi
  [[ "$(file "$object")" == *"Mach-O 64-bit object arm64"* ]] ||
    fail "non-arm64 object: $source"
done < "$closure_manifest"

archive="$stage/libopenjdk-file-input-stream-darwin.a"
"$libtool_bin" -static -o "$archive" "$objects"/*.o
member_count="$({ ar -t "$archive" || true; } | grep -v '^__\.SYMDEF' | wc -l | tr -d ' ')"
[[ "$member_count" == "$FILE_INPUT_STREAM_SOURCE_COUNT" &&
   "$(lipo -archs "$archive")" == arm64 ]] ||
  fail "archive identity members=$member_count archs=$(lipo -archs "$archive")"

symbols="$stage/archive-symbols.txt"
nm -aC "$archive" > "$symbols"
grep -F ' T _register_java_io_FileInputStream' "$symbols" >/dev/null ||
  fail "complete registrar missing"
for method in length0 position0 skip0 available0; do
  grep -F " T _Java_java_io_FileInputStream_${method}" "$symbols" >/dev/null ||
    fail "native definition missing: $method"
done

file_input_available_relocs="$stage/file-input-available-relocs.txt"
file_input_disassembly="$stage/file-input-disassembly.txt"
objdump -d --reloc "$objects/FileInputStream.o" > "$file_input_disassembly"
awk '
  /<_available>:/ { capture = 1 }
  capture && seen && /^[[:xdigit:]]+ <[^>]+>:/ { exit }
  capture { print; seen = 1 }
' "$file_input_disassembly" > "$file_input_available_relocs"
grep -F '_darwin_art_openjdk_nio_ioctl' "$file_input_available_relocs" >/dev/null ||
  fail "FileInputStream available bypassed virtual descriptor ioctl owner"
if grep -E 'ARM64_RELOC_BRANCH26[[:space:]]+_ioctl$' \
  "$file_input_available_relocs" >/dev/null; then
  fail "FileInputStream available retained raw Darwin ioctl"
fi

# available0() is the path SharedPreferences reaches while parsing XML. Audit
# the call sites themselves: archive-wide undefined-symbol checks cannot reject
# the raw Darwin syscalls because the redirect helpers legitimately retain
# those calls for genuine host descriptors.
available_relocs="$stage/handle-available-relocs.txt"
io_util_disassembly="$stage/io-util-disassembly.txt"
objdump -d --reloc "$objects/io_util_md.o" > "$io_util_disassembly"
awk '
  /<_handleAvailable>:/ { capture = 1 }
  capture && seen && /^[[:xdigit:]]+ <[^>]+>:/ { exit }
  capture { print; seen = 1 }
' "$io_util_disassembly" > "$available_relocs"
for owner in fstat ioctl lseek; do
  grep -F "_darwin_art_openjdk_nio_${owner}" \
    "$available_relocs" >/dev/null ||
    fail "handleAvailable bypassed virtual descriptor owner: $owner"
done
if grep -E 'ARM64_RELOC_BRANCH26[[:space:]]+_(fstat|ioctl|lseek)$' \
  "$available_relocs" >/dev/null; then
  fail "handleAvailable retained a raw Darwin descriptor syscall"
fi

# The managed smoke executes inside a host JVM and therefore passes genuine
# Darwin descriptors. Keep a host-only object for that diagnostic; the archive
# installed into Darwin ART remains the virtual-descriptor production variant.
host_file_input_object="$objects/FileInputStream-host.o"
"$cc" "${common_flags[@]}" -c "$native_root/FileInputStream.c" \
  -o "$host_file_input_object"
host_io_util_object="$objects/io_util_md-host.o"
"$cc" "${common_flags[@]}" \
  -c "$host_patched_root/ojluni/src/main/native/io_util_md.c" \
  -o "$host_io_util_object"
host_archive="$stage/libopenjdk-file-input-stream-host-smoke.a"
"$libtool_bin" -static -o "$host_archive" \
  "$host_file_input_object" "$host_io_util_object" \
  "$objects/jni_util.o" "$objects/jni_util_md.o"

probe_object="$objects/android16_file_input_stream_jni.o"
"$cc" "${common_flags[@]}" \
  -c "$project_root/probes/android16_file_input_stream_jni.c" \
  -o "$probe_object"

link_variant() {
  local nativehelper="$1" input_archive="$2" output="$3"
  "$cc" -arch arm64 -isysroot "$sdk_root" -dynamiclib \
    "$probe_object" -Wl,-force_load,"$input_archive" \
    "$openjdkjvm" "$nativehelper" "$liblog" \
    -Wl,-exported_symbol,_JNI_OnLoad -Wl,-dead_strip \
    -Wl,-undefined,dynamic_lookup -framework CoreFoundation -o "$output"
}

device_library="$stage/libfile-input-stream-device-closure.dylib"
link_variant "$device_nativehelper" "$archive" "$device_library"
device_undefined="$stage/device-retained-undefined.txt"
nm -u "$device_library" | sed 's/^[[:space:]]*//' | sort -u > "$device_undefined"
for forbidden in _AFileDescriptor_getFd _jniRegisterNativeMethods \
  _JVM_GetLastErrorString _IO_fd_fdID; do
  if grep -Fx "$forbidden" "$device_undefined" >/dev/null; then
    fail "device provider closure retained $forbidden"
  fi
done

host_library="$stage/libfile-input-stream-managed.dylib"
link_variant "$host_nativehelper" "$host_archive" "$host_library"
classes="$stage/classes"
mkdir -p "$classes"
javac --release 17 -encoding UTF-8 -d "$classes" \
  "$project_root/probes/file-input-stream/FileInputStreamDarwinSmoke.java"
managed_output="$(java --add-opens java.base/java.io=ALL-UNNAMED \
  -cp "$classes" dev.darwinart.probe.FileInputStreamDarwinSmoke "$host_library")"
expected='managed-file-input-stream: methods=4 open=pass read=pass length=pass position=pass available=pass skip=pass close=pass'
[[ "$managed_output" == "$expected" ]] ||
  fail "managed acceptance failed: $managed_output"

duplicates="$stage/duplicate-providers.txt"
for provider in \
  "$project_root/_build/runtime-bootstrap/libart-runtime-bootstrap-darwin.a" \
  "$project_root/_build/runtime-graphics-bootstrap/libart-runtime-graphics-bootstrap-darwin.a" \
  "$project_root/_build/libcore-darwin-linux/libcore-darwin-linux.a"; do
  [[ -f "$provider" ]] || continue
  if nm -aC "$provider" | grep -E \
    ' [TDSB] _(register_java_io_FileInputStream|Java_java_io_FileInputStream_(length0|position0|skip0|available0))$' \
    >> "$duplicates"; then
    fail "duplicate FileInputStream provider in $provider"
  fi
done

undefined="$stage/archive-undefined.txt"
nm -u "$archive" | sed 's/^[[:space:]]*//' | sort -u > "$undefined"
mkdir -p "$build_dir"
cp "$archive" "$build_dir/libopenjdk-file-input-stream-darwin.a"
cp "$method_manifest" "$build_dir/file-input-stream-methods.tsv"
cp "$closure_manifest" "$build_dir/file-input-stream-sources.txt"
cp "$symbols" "$build_dir/archive-symbols.txt"
cp "$undefined" "$build_dir/archive-undefined.txt"
cp "$device_undefined" "$build_dir/device-retained-undefined.txt"
cp "$duplicates" "$build_dir/duplicate-providers.txt"
cp "$patched_root/ojluni/src/main/native/io_util_md.c" \
  "$build_dir/io_util_md.darwin.c"

echo "file-input-stream: sources=$closure_count/$source_count methods=$method_count device_closure=pass managed=open-read-available-skip-close-pass archive=Mach-O-arm64"
