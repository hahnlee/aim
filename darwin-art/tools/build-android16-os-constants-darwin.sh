#!/bin/bash
set -euo pipefail
export LC_ALL=C

mode="${1:---archive-and-test}"
[[ $# -le 1 && ( "$mode" == --archive-only || "$mode" == --archive-and-test ) ]] || {
  echo 'usage: build-android16-os-constants-darwin.sh [--archive-only|--archive-and-test]' >&2
  exit 2
}

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/.." && pwd)"
lock_file="$project_root/upstream/android16-os-constants.lock"
source_root="$project_root/_aosp/libcore-os-constants"
bionic_root="$project_root/_aosp/bionic-os-constants"
build_dir="$project_root/_build/os-constants"

# shellcheck disable=SC1090
source "$lock_file"

sha256() { shasum -a 256 "$1" | awk '{print $1}'; }
fail() { echo "os-constants: $*" >&2; exit 3; }
blocked() { echo "os-constants: BLOCKED: $*" >&2; exit 2; }

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

materialize_bionic_file() {
  local relative="$1" expected="$2"
  local destination="$bionic_root/$relative"
  if [[ -f "$destination" ]]; then
    [[ "$(sha256 "$destination")" == "$expected" ]] ||
      fail "checksum mismatch: $destination"
    return
  fi
  mkdir -p "$(dirname "$destination")"
  local staged
  staged="$(mktemp "${destination}.download.XXXXXX")"
  curl -fsSL \
    "https://android.googlesource.com/$BIONIC_PROJECT/+/$BIONIC_REVISION/$relative?format=TEXT" \
    | base64 -D > "$staged"
  [[ "$(sha256 "$staged")" == "$expected" ]] || {
    rm -f "$staged"
    fail "download checksum mismatch: $relative"
  }
  mv "$staged" "$destination"
}

archive_tree_identity() {
  python3 - "$1" <<'PY'
import hashlib
import sys
import tarfile

archive = tarfile.open(sys.argv[1])
members = sorted((m for m in archive.getmembers() if m.isfile()),
                 key=lambda m: m.name)
digest = hashlib.sha256()
for member in members:
    digest.update(member.name.encode())
    digest.update(b'\0')
    digest.update(archive.extractfile(member).read())
print(f'{len(members)} {digest.hexdigest()}')
PY
}

materialize_bionic_archive() {
  local subtree="$1" expected_count="$2" expected_sha="$3" filename="$4"
  local destination="$bionic_root/archives/$filename"
  if [[ -f "$destination" ]]; then
    [[ "$(archive_tree_identity "$destination")" == \
       "$expected_count $expected_sha" ]] || fail "tree mismatch: $destination"
    return
  fi
  mkdir -p "$(dirname "$destination")"
  local staged
  staged="$(mktemp "${destination}.download.XXXXXX")"
  curl -fsSL \
    "https://android.googlesource.com/$BIONIC_PROJECT/+archive/$BIONIC_REVISION/$subtree.tar.gz" \
    -o "$staged"
  [[ "$(archive_tree_identity "$staged")" == \
     "$expected_count $expected_sha" ]] || {
    rm -f "$staged"
    fail "archive checksum mismatch: $subtree"
  }
  mv "$staged" "$destination"
}

materialize luni/src/main/native/android_system_OsConstants.cpp \
  "$OS_CONSTANTS_CPP_SHA256"
materialize luni/src/main/java/android/system/OsConstants.java \
  "$OS_CONSTANTS_JAVA_SHA256"
materialize luni/src/main/native/Portability.h "$PORTABILITY_H_SHA256"
materialize luni/src/main/native/Android.bp "$NATIVE_ANDROID_BP_SHA256"
materialize luni/src/main/native/Register.cpp "$REGISTER_CPP_SHA256"

materialize_bionic_archive libc/include "$BIONIC_INCLUDE_TREE_FILE_COUNT" \
  "$BIONIC_INCLUDE_TREE_SHA256" libc-include.tar.gz
materialize_bionic_archive libc/kernel/uapi "$BIONIC_UAPI_TREE_FILE_COUNT" \
  "$BIONIC_UAPI_TREE_SHA256" libc-kernel-uapi.tar.gz
materialize_bionic_archive libc/kernel/uapi/asm-arm64 \
  "$BIONIC_ARM64_UAPI_TREE_FILE_COUNT" "$BIONIC_ARM64_UAPI_TREE_SHA256" \
  libc-kernel-uapi-arm64.tar.gz
materialize_bionic_file libc/platform/bionic/reserved_signals.h \
  "$BIONIC_RESERVED_SIGNALS_H_SHA256"
materialize_bionic_file libc/bionic/__libc_current_sigrtmin.cpp \
  "$BIONIC_SIGRTMIN_CPP_SHA256"
materialize_bionic_file libc/bionic/__libc_current_sigrtmax.cpp \
  "$BIONIC_SIGRTMAX_CPP_SHA256"

values="$project_root/upstream/android16-os-constants-values.tsv"
[[ -f "$values" && "$(sha256 "$values")" == "$ANDROID_VALUES_SHA256" ]] ||
  fail "checked Android value manifest mismatch"
[[ "$(wc -l < "$values" | tr -d ' ')" == "$CONSTANT_COUNT" ]] ||
  fail "checked Android value count mismatch"

stage="$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-os-constants.XXXXXX")"
trap 'rm -rf "$stage"' EXIT
native_cpp="$source_root/luni/src/main/native/android_system_OsConstants.cpp"
java_source="$source_root/luni/src/main/java/android/system/OsConstants.java"

python3 - "$native_cpp" "$java_source" "$stage" \
  "$CONSTANT_COUNT" "$CONSTANT_NAMES_SHA256" \
  "$CONSTANT_EXPRESSIONS_SHA256" <<'PY'
import hashlib
import re
import sys
from pathlib import Path

native = Path(sys.argv[1]).read_text()
java = Path(sys.argv[2]).read_text()
out = Path(sys.argv[3])
expected_count = int(sys.argv[4])
expected_names_sha = sys.argv[5]
expected_expressions_sha = sys.argv[6]

entries = re.findall(
    r'^\s*initConstant\(env, c, "([^"]+)", (.*)\);$', native, re.MULTILINE)
names = [name for name, _ in entries]
java_names = re.findall(
    r'^\s*public static final int ([A-Za-z0-9_]+) = placeholder\(\);$',
    java, re.MULTILINE)
names_text = ''.join(name + '\n' for name in names)
expressions_text = ''.join(f'{name}\t{expression}\n' for name, expression in entries)
if len(entries) != expected_count:
    raise SystemExit(f'native constant count drift: {len(entries)}')
if len(java_names) != expected_count or set(java_names) != set(names):
    raise SystemExit('managed/native constant field sets differ')
if hashlib.sha256(names_text.encode()).hexdigest() != expected_names_sha:
    raise SystemExit('constant name manifest drift')
if hashlib.sha256(expressions_text.encode()).hexdigest() != expected_expressions_sha:
    raise SystemExit('constant expression manifest drift')
(out / 'names.txt').write_text(names_text)
(out / 'expressions.tsv').write_text(expressions_text)
PY

grep -F 'android_system_OsConstants.cpp' \
  "$source_root/luni/src/main/native/Android.bp" >/dev/null ||
  fail "OsConstants left libjavacore source selection"
grep -F 'REGISTER(register_android_system_OsConstants);' \
  "$source_root/luni/src/main/native/Register.cpp" >/dev/null ||
  fail "OsConstants registrar left libjavacore registration order"

ndk_root="${ANDROID_NDK_ROOT:-}"
if [[ -z "$ndk_root" && -n "${ANDROID_HOME:-}" ]]; then
  ndk_root="$ANDROID_HOME/ndk/$NDK_REVISION"
fi
[[ -n "$ndk_root" && -d "$ndk_root" ]] ||
  blocked "Android NDK $NDK_REVISION is required via ANDROID_NDK_ROOT or ANDROID_HOME"
[[ "$(sha256 "$ndk_root/source.properties")" == \
   "$NDK_SOURCE_PROPERTIES_SHA256" ]] ||
  fail "NDK revision/provenance mismatch: $ndk_root"
ndk_compiler_types="$ndk_root/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include/linux/compiler_types.h"
[[ -f "$ndk_compiler_types" && "$(sha256 "$ndk_compiler_types")" == \
   "$NDK_COMPILER_TYPES_SHA256" ]] ||
  fail "NDK generated linux/compiler_types.h provenance mismatch"

header_root="$stage/bionic"
mkdir -p "$header_root/libc/include" "$header_root/libc/kernel/uapi" \
  "$header_root/libc/kernel/uapi/asm-arm64"
tar -xzf "$bionic_root/archives/libc-include.tar.gz" \
  -C "$header_root/libc/include"
tar -xzf "$bionic_root/archives/libc-kernel-uapi.tar.gz" \
  -C "$header_root/libc/kernel/uapi"
tar -xzf "$bionic_root/archives/libc-kernel-uapi-arm64.tar.gz" \
  -C "$header_root/libc/kernel/uapi/asm-arm64"

grep -Eq '^#define __SIGRT_RESERVED 10$' \
  "$bionic_root/libc/platform/bionic/reserved_signals.h" ||
  fail "Bionic reserved real-time signal count drift"
grep -F 'return __SIGRTMIN + __SIGRT_RESERVED;' \
  "$bionic_root/libc/bionic/__libc_current_sigrtmin.cpp" >/dev/null ||
  fail "Bionic SIGRTMIN policy drift"
grep -F 'return __SIGRTMAX;' \
  "$bionic_root/libc/bionic/__libc_current_sigrtmax.cpp" >/dev/null ||
  fail "Bionic SIGRTMAX policy drift"

python3 - "$native_cpp" "$stage/expressions.tsv" > "$stage/derive-values.cc" <<'PY'
import sys
from pathlib import Path

native = Path(sys.argv[1]).read_text().splitlines()
expressions = [line.rstrip('\n').split('\t', 1) for line in Path(sys.argv[2]).open()]
for line in native:
    if line.startswith('#include <') and 'nativehelper/' not in line:
        print(line)
print('#include "Portability.h"')
print('extern "C" void derive_android16_values(int* output) {')
for index, (_, expression) in enumerate(expressions):
    print(f'  output[{index}] = static_cast<int>({expression});')
print('}')
PY

clang="$(xcrun --find clang++)"
clang_resource="$($clang -print-resource-dir)"
ndk_sysroot="$ndk_root/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include"
cross_flags=(
  --target=aarch64-linux-android36 -std=c++20 -nostdinc -nostdinc++
  -Wno-macro-redefined
  -I"$header_root/libc/include"
  -I"$header_root/libc/kernel/uapi/asm-arm64"
  -I"$header_root/libc/kernel/uapi"
  -I"$ndk_sysroot/aarch64-linux-android"
  -I"$ndk_sysroot"
  -isystem "$clang_resource/include"
  -I"$source_root/luni/src/main/native"
)
"$clang" "${cross_flags[@]}" -S -emit-llvm -O0 \
  "$stage/derive-values.cc" -o "$stage/derive-values.ll"

python3 - "$stage/expressions.tsv" "$stage/derive-values.ll" \
  > "$stage/derived-values.tsv" <<'PY'
import re
import sys
from pathlib import Path

entries = [line.rstrip('\n').split('\t', 1) for line in Path(sys.argv[1]).open()]
ir = Path(sys.argv[2]).read_text()
stores = re.findall(r'^\s*store i32 ([^,]+), ptr ', ir, re.MULTILINE)
if len(stores) != len(entries):
    raise SystemExit(f'expected {len(entries)} stores, found {len(stores)}')
for (name, _), value in zip(entries, stores):
    if not re.fullmatch(r'-?[0-9]+', value):
        if name == 'SIGRTMIN':
            value = '42'
        elif name == 'SIGRTMAX':
            value = '64'
        else:
            raise SystemExit(f'unexpected dynamic value for {name}: {value}')
    print(f'{name}\t{value}')
PY
cmp -s "$stage/derived-values.tsv" "$values" ||
  fail "Bionic Android 16 cross-derived values differ from locked manifest"
[[ "$(sha256 "$stage/derived-values.tsv")" == "$ANDROID_VALUES_SHA256" ]] ||
  fail "cross-derived value manifest hash mismatch"

python3 - "$values" "$stage" "$ERRNO_CONSTANT_COUNT" \
  "$SYSCONF_CONSTANT_COUNT" <<'PY'
import re
import sys
from pathlib import Path

entries = [line.rstrip('\n').split('\t') for line in Path(sys.argv[1]).open()]
stage = Path(sys.argv[2])
errnos = [(n, v) for n, v in entries
          if re.fullmatch(r'E[A-Z0-9_]+', n)
          and not n.startswith(('EAI_', 'ETH_', 'EXIT_'))]
sysconfs = [(n, v) for n, v in entries if n.startswith('_SC_')]
if len(errnos) != int(sys.argv[3]):
    raise SystemExit(f'errno count drift: {len(errnos)}')
if len(sysconfs) != int(sys.argv[4]):
    raise SystemExit(f'sysconf count drift: {len(sysconfs)}')
(stage / 'android16_os_constants_values.inc').write_text(
    ''.join(f'    {{"{n}", {v}}},\n' for n, v in entries))
(stage / 'android16_os_constants_errno.inc').write_text(''.join(
    f'#ifdef {n}\n  if (darwin_errno == {n}) {{ *android_errno = {v}; return true; }}\n#endif\n'
    for n, v in errnos))
(stage / 'android16_os_constants_sysconf.inc').write_text(''.join(
    f'#ifdef {n}\n  if (android_name == {v}) {{ *darwin_name = {n}; return true; }}\n#endif\n'
    for n, v in sysconfs))
PY

nativehelper_source="$project_root/_aosp/libnativehelper-full"
for required in \
  "$nativehelper_source/include_jni/jni.h"; do
  [[ -e "$required" ]] || blocked "missing build dependency: $required"
done

cc="$(xcrun --find clang++)"
libtool_bin="$(xcrun --find libtool)"
sdk_root="$(xcrun --sdk macosx --show-sdk-path)"
common_flags=(
  -std=c++20 -arch arm64 -isysroot "$sdk_root" -fPIC
  -Wall -Wextra -Werror
  -I"$project_root/compat" -I"$stage"
  -I"$nativehelper_source/include_jni"
  -I"$nativehelper_source/include"
  -I"$nativehelper_source/include_platform"
  -I"$nativehelper_source/include_platform_header_only"
  -I"$nativehelper_source/header_only_include"
)
"$cc" "${common_flags[@]}" -c "$project_root/compat/darwin_os_constants.cc" \
  -o "$stage/darwin_os_constants.o"
[[ "$(file "$stage/darwin_os_constants.o")" == *"Mach-O 64-bit object arm64"* ]] ||
  fail "OsConstants object is not Darwin arm64"
archive="$stage/libandroid-system-os-constants-darwin.a"
"$libtool_bin" -static -o "$archive" "$stage/darwin_os_constants.o"
[[ "$(lipo -archs "$archive")" == arm64 ]] || fail "archive is not arm64"
[[ "$({ ar -t "$archive" || true; } | grep -v '^__\.SYMDEF' | wc -l | tr -d ' ')" == 1 ]] ||
  fail "archive must contain the complete one-TU registrar module"
nm -g "$archive" | grep -F 'register_android_system_OsConstants' >/dev/null ||
  fail "complete OsConstants registrar symbol missing"

mkdir -p "$build_dir" "$build_dir/generated"
cp "$stage/android16_os_constants_values.inc" \
  "$stage/android16_os_constants_errno.inc" \
  "$stage/android16_os_constants_sysconf.inc" "$build_dir/generated/"
cp "$values" "$build_dir/android16-os-constants-values.tsv"
cp "$stage/names.txt" "$build_dir/generated/names.txt"
cp "$stage/expressions.tsv" "$build_dir/generated/expressions.tsv"
cp "$stage/derived-values.tsv" "$build_dir/generated/derived-values.tsv"
cp "$archive" "$build_dir/libandroid-system-os-constants-darwin.a"
echo "os-constants: archive=Mach-O-arm64 fields=$CONSTANT_COUNT Android-ABI=pass source-derived=pass"
if [[ "$mode" == --archive-and-test ]]; then
  bash "$script_dir/test-android16-os-constants-darwin.sh" --prepared
fi
