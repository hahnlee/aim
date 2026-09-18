#!/bin/bash
set -euo pipefail
export LC_ALL=C

mode="${1:---archive-and-test}"
[[ $# -le 1 && ( "$mode" == --archive-only || "$mode" == --archive-and-test ) ]] || {
  echo 'usage: build-android16-unix-filesystem-darwin.sh [--archive-only|--archive-and-test]' >&2
  exit 2
}

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/.." && pwd)"
lock_file="$project_root/upstream/android16-unix-filesystem-darwin.lock"
source_root="$project_root/_aosp/libcore-unix-filesystem"
native_root="$source_root/ojluni/src/main/native"
build_dir="$project_root/_build/unix-filesystem-darwin"

# shellcheck disable=SC1090
source "$lock_file"

sha256() { shasum -a 256 "$1" | awk '{print $1}'; }
fail() { echo "unix-filesystem: $*" >&2; exit 3; }

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
  [[ "$(sha256 "$staged")" == "$expected" ]] || {
    rm -f "$staged"
    fail "download checksum mismatch: $relative"
  }
  mv "$staged" "$destination"
}

materialize NativeCode.bp "$NATIVE_CODE_BP_SHA256"
materialize ojluni/src/main/native/Android.bp "$NATIVE_ANDROID_BP_SHA256"
materialize ojluni/src/main/native/OnLoad.cpp "$ONLOAD_CPP_SHA256"
materialize ojluni/src/main/native/UnixFileSystem_md.c "$UNIX_FILESYSTEM_C_SHA256"
materialize ojluni/src/main/native/canonicalize_md.c "$CANONICALIZE_C_SHA256"
materialize ojluni/src/main/native/classfile_constants.h \
  "$CLASSFILE_CONSTANTS_H_SHA256"
materialize ojluni/src/main/native/jni_util.c "$JNI_UTIL_C_SHA256"
materialize ojluni/src/main/native/jni_util_md.c "$JNI_UTIL_MD_C_SHA256"
materialize ojluni/src/main/native/io_util_md.c "$IO_UTIL_MD_C_SHA256"
materialize ojluni/src/main/native/jni_util.h "$JNI_UTIL_H_SHA256"
materialize ojluni/src/main/native/jlong.h "$JLONG_H_SHA256"
materialize ojluni/src/main/native/jlong_md.h "$JLONG_MD_H_SHA256"
materialize ojluni/src/main/native/jvm.h "$JVM_H_SHA256"
materialize ojluni/src/main/native/jvm_md.h "$JVM_MD_H_SHA256"
materialize ojluni/src/main/native/io_util.h "$IO_UTIL_H_SHA256"
materialize ojluni/src/main/native/io_util_md.h "$IO_UTIL_MD_H_SHA256"
materialize ojluni/src/main/native/java_io_FileSystem.h "$JAVA_IO_FILESYSTEM_H_SHA256"
materialize ojluni/src/main/native/java_io_UnixFileSystem.h \
  "$JAVA_IO_UNIX_FILESYSTEM_H_SHA256"

stage="$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-unixfs.XXXXXX")"
trap 'rm -rf "$stage"' EXIT

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

closure_manifest="$stage/unix-filesystem-closure-sources.txt"
python3 - "$source_manifest" > "$closure_manifest" <<'PY'
import sys
from pathlib import Path

sources = set(Path(sys.argv[1]).read_text().splitlines())
closure = [
    "UnixFileSystem_md.c",
    "canonicalize_md.c",
    "jni_util.c",
    "jni_util_md.c",
    "io_util_md.c",
]
missing = [source for source in closure if source not in sources]
if missing:
    raise SystemExit(f"support sources left libopenjdk_native_srcs: {missing}")
print(*closure, sep="\n")
PY
closure_count="$(wc -l < "$closure_manifest" | tr -d ' ')"
closure_sha="$(sha256 "$closure_manifest")"
[[ "$closure_count" == "$UNIX_FILESYSTEM_CLOSURE_SOURCE_COUNT" &&
   "$closure_sha" == "$UNIX_FILESYSTEM_CLOSURE_SOURCE_LIST_SHA256" ]] ||
  fail "closure identity count=$closure_count sha=$closure_sha"

method_manifest="$stage/unix-filesystem-methods.tsv"
python3 - "$native_root/UnixFileSystem_md.c" > "$method_manifest" <<'PY'
import re
import sys
from pathlib import Path

text = Path(sys.argv[1]).read_text()
start = text.index("static JNINativeMethod gMethods[]")
end = text.index("void register_java_io_UnixFileSystem", start)
methods = re.findall(
    r'NATIVE_METHOD\(UnixFileSystem,\s*([A-Za-z0-9_]+),\s*"([^"]+)"\)',
    text[start:end],
)
for name, signature in methods:
    print(f"{name}\t{signature}")
PY
method_count="$(wc -l < "$method_manifest" | tr -d ' ')"
method_sha="$(sha256 "$method_manifest")"
[[ "$method_count" == "$UNIX_FILESYSTEM_METHOD_COUNT" &&
   "$method_sha" == "$UNIX_FILESYSTEM_METHOD_MANIFEST_SHA256" ]] ||
  fail "method table drift count=$method_count sha=$method_sha"
grep -Fx $'list0\t(Ljava/io/File;)[Ljava/lang/String;' "$method_manifest" >/dev/null ||
  fail "list0 signature missing"

grep -F 'register_java_io_UnixFileSystem' "$native_root/OnLoad.cpp" >/dev/null ||
  fail "OnLoad registrar declaration/call missing"
grep -F 'register_java_io_UnixFileSystem(env);' "$native_root/OnLoad.cpp" >/dev/null ||
  fail "OnLoad registrar order call missing"
python3 - "$source_root/NativeCode.bp" <<'PY'
import sys
from pathlib import Path

text = Path(sys.argv[1]).read_text()
assert 'name: "libopenjdk_native_defaults"' in text
assert 'srcs: [":libopenjdk_native_srcs"]' in text
assert 'target: {\n        darwin: {\n            enabled: false,' in text
PY

nativehelper="$project_root/_build/nativehelper-device-foundation"
nativehelper_source="$project_root/_aosp/libnativehelper-full"
nativehelper_archive="$nativehelper/libnativehelper-device-darwin.a"
liblog_include="$project_root/_aosp/system/logging/liblog/include"
liblog_archive="$project_root/_build/graphics-foundations/liblog-darwin.a"
for required in \
  "$nativehelper_source/include_jni/jni.h" \
  "$nativehelper_source/include/nativehelper/JNIHelp.h" \
  "$nativehelper_archive" \
  "$liblog_include/android/log.h" \
  "$liblog_archive" \
  "$project_root/compat/darwin_libcore_filesystem_bridge.c" \
  "$project_root/compat/darwin_libcore_filesystem_bridge.h"; do
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
  -DMACOSX -D_ALLBSD_SOURCE
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
  -I"$project_root/compat"
  -I"$project_root/tools/bionic-fs-facade/include"
  -I"$project_root/tools/bionic-ioctl-facade/include"
  -I"$project_root/tools/bionic-errno-tls/include"
)

while IFS= read -r source; do
  object="$objects/${source%.*}.o"
  "$cc" "${common_flags[@]}" \
    -include "$project_root/compat/darwin_libcore_filesystem_bridge.h" \
    -DDARWIN_ART_LIBCORE_REDIRECT_FILESYSTEM \
    -c "$native_root/$source" -o "$object"
  [[ "$(file "$object")" == *"Mach-O 64-bit object arm64"* ]] ||
    fail "non-arm64 object: $source"
done < "$closure_manifest"

for bridge in stat statfs mkdir chmod opendir readdir closedir; do
  nm -u "$objects/UnixFileSystem_md.o" | grep -F "_darwin_art_libcore_$bridge" >/dev/null ||
    fail "UnixFileSystem redirect not linked: $bridge"
done
nm -u "$objects/canonicalize_md.o" | grep -F \
  '_darwin_art_libcore_realpath' >/dev/null ||
  fail "canonicalize redirect not linked: realpath"
for bridge in fstat open; do
  nm -u "$objects/io_util_md.o" | grep -F "_darwin_art_libcore_$bridge" >/dev/null ||
    fail "io_util redirect not linked: $bridge"
done
{ nm -u "$objects/io_util_md.o"; nm -u "$objects/UnixFileSystem_md.o"; } |
  grep -F '_darwin_art_libcore_close' >/dev/null ||
  fail "filesystem close redirect not linked"

"$cc" "${common_flags[@]}" \
  -c "$project_root/compat/darwin_libcore_filesystem_bridge.c" \
  -o "$objects/darwin_libcore_filesystem_bridge.o"

archive="$stage/libopenjdk-unix-filesystem-darwin.a"
"$libtool_bin" -static -o "$archive" "$objects"/*.o
member_count="$({ ar -t "$archive" || true; } | grep -v '^__\.SYMDEF' | wc -l | tr -d ' ')"
[[ "$member_count" == "$((UNIX_FILESYSTEM_CLOSURE_SOURCE_COUNT + 1))" &&
   "$(lipo -archs "$archive")" == arm64 ]] ||
  fail "archive identity members=$member_count archs=$(lipo -archs "$archive")"

symbols="$stage/archive-symbols.txt"
nm -aC "$archive" > "$symbols"
grep -F ' T _register_java_io_UnixFileSystem' "$symbols" >/dev/null ||
  fail "complete registrar definition missing"
for method in initIDs canonicalize0 getBooleanAttributes0 getNameMax0 \
  setPermission0 getLastModifiedTime0 createFileExclusively0 list0 \
  createDirectory0 setLastModifiedTime0 setReadOnly0 getSpace0; do
  grep -F " T _Java_java_io_UnixFileSystem_${method}" "$symbols" >/dev/null ||
    fail "native definition missing: $method"
done

undefined="$stage/archive-undefined.txt"
nm -u "$archive" | sed 's/^[[:space:]]*//' | sort -u > "$undefined"
mkdir -p "$build_dir"
cp "$archive" "$build_dir/libopenjdk-unix-filesystem-darwin.a"
cp "$method_manifest" "$build_dir/unix-filesystem-methods.tsv"
cp "$closure_manifest" "$build_dir/unix-filesystem-closure-sources.txt"
cp "$symbols" "$build_dir/archive-symbols.txt"
cp "$undefined" "$build_dir/archive-undefined.txt"
echo "unix-filesystem: sources=$closure_count/$source_count methods=$method_count archive=Mach-O-arm64"
if [[ "$mode" == --archive-and-test ]]; then
  bash "$script_dir/test-android16-unix-filesystem-darwin.sh" --prepared
fi
