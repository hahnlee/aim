#!/bin/bash
set -euo pipefail
export LC_ALL=C

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/.." && pwd)"
output="${1:-}"
[[ "$output" = /* ]] || { echo "openjdk-named-jni: output must be absolute" >&2; exit 2; }

fail() { echo "openjdk-named-jni: $*" >&2; exit 3; }
sha256() { shasum -a 256 "$1" | awk '{print $1}'; }

fis_archive="$project_root/_build/file-input-stream-darwin/libopenjdk-file-input-stream-darwin.a"
ufs_archive="$project_root/_build/unix-filesystem-darwin/libopenjdk-unix-filesystem-darwin.a"
und_archive="$project_root/_build/unix-native-dispatcher-darwin/libopenjdk-unix-native-dispatcher-darwin.a"
fd_archive="$project_root/_build/file-descriptor-darwin/libopenjdk-file-descriptor-darwin.a"
errno_archive="$project_root/_build/bionic-runtime-provider-closure/libaim-bionic-float-conversion.a"
nativehelper_archive="$project_root/_build/nativehelper-device-foundation/libnativehelper-device-darwin.a"
liblog_archive="$project_root/_build/graphics-foundations/liblog-darwin.a"
for archive in "$fis_archive" "$ufs_archive" "$und_archive" "$fd_archive" \
    "$errno_archive" "$nativehelper_archive" "$liblog_archive"; do
  [[ -f "$archive" ]] || fail "missing AOSP OpenJDK archive: $archive"
done

source_fis="$project_root/_aosp/libcore-file-input-stream/ojluni/src/main/native/FileInputStream.c"
source_ufs="$project_root/_aosp/libcore-unix-filesystem/ojluni/src/main/native/UnixFileSystem_md.c"
source_und="$project_root/_aosp/libcore-unix-native-dispatcher/ojluni/src/main/native/UnixNativeDispatcher.c"
copy_source="$project_root/compat/darwin_openjdk_nio_copy.c"
for source in "$source_fis" "$source_ufs" "$source_und" "$copy_source"; do
  [[ -f "$source" ]] || fail "missing named-JNI source provenance: $source"
done

stage="$(mktemp -d "${TMPDIR:-/tmp}/aim-openjdk-named-jni.XXXXXX")"
trap 'rm -rf "$stage"' EXIT
source_manifest="$stage/source-symbols.txt"
archive_manifest="$stage/archive-symbols.txt"
export_manifest="$stage/exported-symbols.txt"

# Derive the expected names from the AOSP RegisterNatives tables, not from a
# hand-maintained allowlist. The table names are the exact symbols that ART
# tries after RegisterNatives has not installed a method. UnixCopyFile is the
# one AOSP named-JNI implementation retained as a direct entrypoint on Darwin.
python3 - "$source_fis" "$source_ufs" "$source_und" "$copy_source" > "$source_manifest" <<'PY'
import re
import sys
from pathlib import Path

def table_symbols(path, prefix):
    text = Path(path).read_text()
    start = text.index('static JNINativeMethod gMethods[]')
    body = text[start:text.index('};', start)]
    return {f'{prefix}_{name}' for _, name in re.findall(
        r'NATIVE_METHOD\(\s*([^,]+),\s*([A-Za-z0-9_]+),', body)}

symbols = set()
symbols |= table_symbols(sys.argv[1], 'Java_java_io_FileInputStream')
symbols |= table_symbols(sys.argv[2], 'Java_java_io_UnixFileSystem')
symbols |= table_symbols(sys.argv[3], 'Java_sun_nio_fs_UnixNativeDispatcher')
copy_text = Path(sys.argv[4]).read_text()
symbols |= set(re.findall(r'\b(Java_sun_nio_fs_UnixCopyFile_[A-Za-z0-9_]+)\s*\(', copy_text))
print(*sorted(symbols), sep='\n')
PY

[[ "$(wc -l < "$source_manifest" | tr -d ' ')" == 64 ]] ||
  fail "AOSP named-JNI source manifest count drift"

{
  nm -gU "$fis_archive"
  nm -gU "$ufs_archive"
  nm -gU "$und_archive"
} | awk '/_Java_[A-Za-z0-9_]+$/ { sub(/^.* _/, ""); print }' | sort -u \
  > "$archive_manifest"
[[ "$(wc -l < "$archive_manifest" | tr -d ' ')" == 64 ]] ||
  fail "AOSP named-JNI archive manifest count drift"

if ! cmp -s "$source_manifest" "$archive_manifest"; then
  fail "AOSP source/archive named-JNI manifest mismatch\nsource-only: $(comm -23 "$source_manifest" "$archive_manifest" | tr '\n' ' ')\narchive-only: $(comm -13 "$source_manifest" "$archive_manifest" | tr '\n' ' ')"
fi
sed 's/^/_/' "$archive_manifest" > "$export_manifest"

cc="$(xcrun --find clang++)"
sdk_root="$(xcrun --sdk macosx --show-sdk-path)"
mkdir -p "$(dirname "$output")"
mkdir -p "$stage/fis" "$stage/ufs" "$stage/und" "$stage/fd" "$stage/errno" "$stage/support"
# The three AOSP module closures each carry private copies of jni_util/io_util.
# Link one support closure and only the named-JNI implementation members from
# the other modules; force-loading whole archives would create duplicate
# private owners and obscure which source module owns each symbol.
(cd "$stage/fis" && ar -x "$fis_archive" FileInputStream.o io_util_md.o jni_util.o jni_util_md.o)
(cd "$stage/ufs" && ar -x "$ufs_archive" UnixFileSystem_md.o canonicalize_md.o darwin_libcore_filesystem_bridge.o)
(cd "$stage/und" && ar -x "$und_archive" UnixNativeDispatcher.o darwin_openjdk_nio_copy.o)
# FileInputStream and the NIO dispatcher share the AOSP FileDescriptor field
# cache.  The runtime image deliberately does not export private C globals,
# so the RTLD_LOCAL named-JNI owner must carry its real module owner rather
# than leaving IO_fd_fdID as a flat-namespace lookup.
(cd "$stage/fd" && ar -x "$fd_archive" FileDescriptor_md.o)
(cd "$stage/errno" && ar -x "$errno_archive" errno_tls_c.o)
(cd "$stage/support" && ar -x "$nativehelper_archive" JniConstants.o JNIHelp.o ExpandableString.o file_descriptor_jni.o)
(cd "$stage/support" && ar -x "$liblog_archive" logger_write.o properties.o)
"$cc" -arch arm64 -isysroot "$sdk_root" -dynamiclib \
  -Wl,-exported_symbols_list,"$export_manifest" \
  -Wl,-undefined,dynamic_lookup \
  "$stage/fis/FileInputStream.o" "$stage/fis/io_util_md.o" \
  "$stage/fis/jni_util.o" "$stage/fis/jni_util_md.o" \
  "$stage/ufs/UnixFileSystem_md.o" "$stage/ufs/canonicalize_md.o" \
  "$stage/ufs/darwin_libcore_filesystem_bridge.o" \
  "$stage/und/UnixNativeDispatcher.o" "$stage/und/darwin_openjdk_nio_copy.o" \
  "$stage/fd/FileDescriptor_md.o" \
  "$stage/errno/errno_tls_c.o" \
  "$stage/support/JniConstants.o" "$stage/support/JNIHelp.o" \
  "$stage/support/ExpandableString.o" "$stage/support/file_descriptor_jni.o" \
  "$stage/support/logger_write.o" "$stage/support/properties.o" \
  \
  -Wl,-u,___android_log_is_loggable \
  -o "$output"
[[ "$(file "$output")" == *"Mach-O 64-bit dynamically linked shared library arm64"* ]] ||
  fail "named-JNI owner is not a Darwin arm64 dylib"

actual_manifest="$stage/actual-symbols.txt"
nm -gU "$output" | awk '/_Java_[A-Za-z0-9_]+$/ { sub(/^.* _/, ""); print }' | sort -u \
  > "$actual_manifest"
if ! cmp -s "$archive_manifest" "$actual_manifest"; then
  fail "named-JNI owner exported-symbol manifest mismatch\nmissing: $(comm -23 "$archive_manifest" "$actual_manifest" | tr '\n' ' ')\nsurplus: $(comm -13 "$archive_manifest" "$actual_manifest" | tr '\n' ' ')"
fi

# AOSP owns the JVM_* support ABI in the distinct libopenjdkjvm shared module;
# libopenjdk merely imports it.  Preserve that boundary here: the process
# runtime publishes the real pinned module and this RTLD_LOCAL image retains
# its relocation instead of cloning OpenjdkJvm.cc into a second owner.
owner_undefined="$stage/owner-undefined.txt"
nm -u "$output" | sed 's/^[[:space:]]*//' | sort -u > "$owner_undefined"
grep -Fx '_JVM_GetLastErrorString' "$owner_undefined" >/dev/null ||
  fail "named-JNI owner did not retain libopenjdkjvm JVM_GetLastErrorString import"
if nm -a "$output" | awk '$NF ~ /^_(JVM_|jio_)/ && $(NF-1) != "U" { found=1 } END { exit !found }'; then
  fail "named-JNI owner contains a duplicate libopenjdkjvm definition"
fi

manifest_dir="$project_root/_build/system-natives-darwin"
mkdir -p "$manifest_dir"
cp "$source_manifest" "$manifest_dir/openjdk-named-jni-source-symbols.txt"
cp "$archive_manifest" "$manifest_dir/openjdk-named-jni-archive-symbols.txt"
cp "$export_manifest" "$manifest_dir/openjdk-named-jni-exported-symbols.txt"
cp "$actual_manifest" "$manifest_dir/openjdk-named-jni-owner-symbols.txt"
printf '%s\n' "source_sha256=$(sha256 "$source_manifest")" \
  "archive_sha256=$(sha256 "$archive_manifest")" \
  "exported_sha256=$(sha256 "$export_manifest")" \
  "owner_sha256=$(sha256 "$actual_manifest")" \
  "exports=64" > "$manifest_dir/openjdk-named-jni-owner-manifest.txt"

echo "openjdk-named-jni: exports=64 JNI_OnLoad=0 source/archive=exact JVM-owner=external RTLD_LOCAL=caller"
