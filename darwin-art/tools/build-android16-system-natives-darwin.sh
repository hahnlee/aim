#!/bin/bash
set -euo pipefail
export LC_ALL=C

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/.." && pwd)"
lock_file="$project_root/upstream/android16-system-natives-darwin.lock"
source_root="$project_root/_aosp/libcore-system-natives"
native_root="$source_root/libcore/ojluni/src/main/native"
build_dir="$project_root/_build/system-natives-darwin"
patch_file="$project_root/patches/libcore-openjdk/0004-darwin-system-boringssl-version-header.patch"
library_name_patch="$project_root/patches/libcore-openjdk/0005-android-guest-jni-library-suffix.patch"
working_directory_patch="$project_root/patches/libcore-openjdk/0006-darwin-system-guest-working-directory.patch"
boringssl_patch="$project_root/patches/boringssl/0001-darwin-cxx17.patch"

# shellcheck disable=SC1090
source "$lock_file"

sha256() { shasum -a 256 "$1" | awk '{print $1}'; }
fail() { echo "system-natives: $*" >&2; exit 3; }

materialize() {
  local project="$1" revision="$2" relative="$3" expected="$4" destination="$source_root/$5"
  if [[ -f "$destination" ]]; then
    [[ "$(sha256 "$destination")" == "$expected" ]] ||
      fail "checksum mismatch: $destination"
    return
  fi
  mkdir -p "$(dirname "$destination")"
  local staged
  staged="$(mktemp "${destination}.download.XXXXXX")"
  curl -fsSL \
    "https://android.googlesource.com/$project/+/$revision/$relative?format=TEXT" \
    | base64 -D > "$staged"
  [[ "$(sha256 "$staged")" == "$expected" ]] ||
    fail "download checksum mismatch: $project/$relative"
  mv "$staged" "$destination"
}

materialize "$LIBCORE_PROJECT" "$LIBCORE_REVISION" \
  ojluni/src/main/native/Android.bp "$NATIVE_ANDROID_BP_SHA256" \
  libcore/ojluni/src/main/native/Android.bp
materialize "$LIBCORE_PROJECT" "$LIBCORE_REVISION" NativeCode.bp \
  "$NATIVE_CODE_BP_SHA256" libcore/NativeCode.bp
materialize "$LIBCORE_PROJECT" "$LIBCORE_REVISION" \
  ojluni/src/main/native/System.c "$SYSTEM_C_SHA256" \
  libcore/ojluni/src/main/native/System.c
materialize "$LIBCORE_PROJECT" "$LIBCORE_REVISION" \
  ojluni/src/main/java/java/lang/System.java "$SYSTEM_JAVA_SHA256" \
  libcore/ojluni/src/main/java/java/lang/System.java
materialize "$LIBCORE_PROJECT" "$LIBCORE_REVISION" \
  ojluni/src/main/native/jdk_internal_misc_VM.cpp \
  "$JDK_INTERNAL_MISC_VM_CPP_SHA256" \
  libcore/ojluni/src/main/native/jdk_internal_misc_VM.cpp
materialize "$LIBCORE_PROJECT" "$LIBCORE_REVISION" \
  luni/src/main/native/java_lang_invoke_MethodHandle.cpp \
  "$METHOD_HANDLE_CPP_SHA256" \
  libcore/luni/src/main/native/java_lang_invoke_MethodHandle.cpp
materialize "$LIBCORE_PROJECT" "$LIBCORE_REVISION" \
  luni/src/main/native/java_lang_invoke_VarHandle.cpp \
  "$VAR_HANDLE_CPP_SHA256" \
  libcore/luni/src/main/native/java_lang_invoke_VarHandle.cpp
materialize "$ART_PROJECT" "$ART_REVISION" runtime/native/java_lang_System.cc \
  "$ART_SYSTEM_CC_SHA256" art/runtime/native/java_lang_System.cc
materialize "$BORINGSSL_PROJECT" "$BORINGSSL_REVISION" \
  src/include/openssl/crypto.h "$BORINGSSL_CRYPTO_H_SHA256" \
  boringssl/src/include/openssl/crypto.h
materialize "$LIBCORE_PROJECT" "$LIBCORE_REVISION" \
  luni/src/main/native/libcore_math_NativeBN.cpp "$NATIVE_BN_CPP_SHA256" \
  libcore/luni/src/main/native/libcore_math_NativeBN.cpp
materialize "$LIBCORE_PROJECT" "$LIBCORE_REVISION" \
  luni/src/main/native/JniException.h "$JNI_EXCEPTION_H_SHA256" \
  libcore/luni/src/main/native/JniException.h
materialize "$LIBCORE_PROJECT" "$LIBCORE_REVISION" \
  luni/src/main/native/JniException.cpp "$JNI_EXCEPTION_CPP_SHA256" \
  libcore/luni/src/main/native/JniException.cpp
materialize "$LIBCORE_PROJECT" "$LIBCORE_REVISION" \
  luni/src/main/native/libcore_icu_ICU.cpp "$LIBCORE_ICU_CPP_SHA256" \
  libcore/luni/src/main/native/libcore_icu_ICU.cpp
materialize "$LIBCORE_PROJECT" "$LIBCORE_REVISION" \
  luni/src/main/native/IcuUtilities.h "$ICU_UTILITIES_H_SHA256" \
  libcore/luni/src/main/native/IcuUtilities.h
materialize "$LIBCORE_PROJECT" "$LIBCORE_REVISION" \
  luni/src/main/native/ScopedIcuULoc.h "$SCOPED_ICU_ULOC_H_SHA256" \
  libcore/luni/src/main/native/ScopedIcuULoc.h
for header_and_hash in \
  "jni_util.c:$JNI_UTIL_C_SHA256" \
  "jni_util_md.c:$JNI_UTIL_MD_C_SHA256" \
  "jni_util.h:$JNI_UTIL_H_SHA256" \
  "jlong.h:$JLONG_H_SHA256" \
  "jlong_md.h:$JLONG_MD_H_SHA256" \
  "jvm.h:$JVM_H_SHA256" \
  "jvm_md.h:$JVM_MD_H_SHA256" \
  "classfile_constants.h:$CLASSFILE_CONSTANTS_H_SHA256" \
  "io_util.h:$IO_UTIL_H_SHA256" \
  "io_util_md.h:$IO_UTIL_MD_H_SHA256"; do
  materialize "$LIBCORE_PROJECT" "$LIBCORE_REVISION" \
    "ojluni/src/main/native/${header_and_hash%%:*}" "${header_and_hash#*:}" \
    "libcore/ojluni/src/main/native/${header_and_hash%%:*}"
done

zlib_header="$project_root/_aosp/external/zlib/zlib.h"
[[ -f "$zlib_header" && "$(sha256 "$zlib_header")" == "$ZLIB_H_SHA256" ]] ||
  fail "pinned Android 16 zlib header missing or changed"
grep -F '#define OPENSSL_VERSION_TEXT "OpenSSL 1.1.1 (compatible; BoringSSL)"' \
  "$source_root/boringssl/src/include/openssl/crypto.h" >/dev/null ||
  fail "pinned BoringSSL compatibility version drift"
[[ "$(sha256 "$patch_file")" == "$DARWIN_PATCH_SHA256" ]] ||
  fail "Darwin BoringSSL header patch checksum mismatch"
[[ "$(sha256 "$library_name_patch")" == "$ANDROID_LIBRARY_NAME_PATCH_SHA256" ]] ||
  fail "Android JNI library-name patch checksum mismatch"
[[ "$(sha256 "$working_directory_patch")" == "$GUEST_WORKING_DIRECTORY_PATCH_SHA256" ]] ||
  fail "guest working-directory patch checksum mismatch"
[[ "$(sha256 "$boringssl_patch")" == "$BORINGSSL_DARWIN_PATCH_SHA256" ]] ||
  fail "Darwin BoringSSL C++ standard patch checksum mismatch"

boringssl_source="$project_root/_aosp/boringssl-full"
[[ -d "$boringssl_source/.git" ]] ||
  fail "missing pinned BoringSSL checkout: $boringssl_source"
[[ "$(git -C "$boringssl_source" rev-parse HEAD)" == "$BORINGSSL_REVISION" ]] ||
  fail "BoringSSL checkout revision drift"
boringssl_cmake_sha="$(git -C "$boringssl_source" show HEAD:CMakeLists.txt | shasum -a 256 | awk '{print $1}')"
[[ "$boringssl_cmake_sha" == "$BORINGSSL_CMAKE_SHA256" ]] ||
  fail "BoringSSL CMake source drift"
boringssl_build="$project_root/_build/boringssl-android16-darwin-repro"
boringssl_overlay="$boringssl_build/source"
mkdir -p "$boringssl_overlay"
git -C "$boringssl_source" show HEAD:CMakeLists.txt > "$boringssl_overlay/CMakeLists.txt"
ln -sfn "$boringssl_source/src" "$boringssl_overlay/src"
patch --batch --forward -p1 -d "$boringssl_overlay" < "$boringssl_patch" >/dev/null
android_sdk="${ANDROID_SDK_ROOT:-${ANDROID_HOME:-$HOME/Library/Android/sdk}}"
cmake_bin="$android_sdk/cmake/3.22.1/bin/cmake"
[[ -x "$cmake_bin" ]] || fail "Android SDK CMake 3.22.1 is missing"
"$cmake_bin" -G Ninja -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_OSX_ARCHITECTURES=arm64 -S "$boringssl_overlay" \
  -B "$boringssl_build/build" >/dev/null
ninja -C "$boringssl_build/build" crypto >/dev/null
boringssl_crypto="$boringssl_build/build/libcrypto.a"
[[ -f "$boringssl_crypto" && "$(lipo -archs "$boringssl_crypto")" == arm64 ]] ||
  fail "BoringSSL crypto archive is not Darwin arm64"

stage="$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-system.XXXXXX")"
trap 'rm -rf "$stage"' EXIT
patched_root="$stage/source"
mkdir -p "$patched_root/ojluni/src/main/native"
cp "$native_root/System.c" "$patched_root/ojluni/src/main/native/System.c"
cp "$native_root/jvm_md.h" "$patched_root/ojluni/src/main/native/jvm_md.h"
patch --batch --forward -p1 -d "$patched_root" < "$patch_file" >/dev/null
patch --batch --forward -p1 -d "$patched_root" < "$library_name_patch" >/dev/null
patch --batch --forward -p1 -d "$patched_root" < "$working_directory_patch" >/dev/null
patched_source="$patched_root/ojluni/src/main/native/System.c"
patched_jvm_md="$patched_root/ojluni/src/main/native/jvm_md.h"
[[ "$(sha256 "$patched_source")" == "$PATCHED_SYSTEM_C_SHA256" ]] ||
  fail "patched System.c checksum mismatch"
[[ "$(sha256 "$patched_jvm_md")" == "$PATCHED_JVM_MD_H_SHA256" ]] ||
  fail "patched jvm_md.h checksum mismatch"

python3 - "$native_root/Android.bp" <<'PY'
import re
import sys
from pathlib import Path

text = Path(sys.argv[1]).read_text()
start = text.index('name: "libopenjdk_native_srcs"')
body_start = text.index('srcs: [', start)
body_end = text.index('],', body_start)
sources = re.findall(r'"([^" ]+\.(?:c|cpp))"', text[body_start:body_end])
if len(sources) != 59 or 'System.c' not in sources:
    raise SystemExit(f'libopenjdk System source membership drift: count={len(sources)}')
PY
for dependency in '"libz"' '"libcrypto_static"' '"libopenjdkjvm"'; do
  grep -F "$dependency" "$source_root/libcore/NativeCode.bp" >/dev/null ||
    fail "libopenjdk dependency drift: $dependency"
done

libcore_methods="$stage/libcore-system-methods.tsv"
python3 - "$native_root/System.c" > "$libcore_methods" <<'PY'
import re
import sys
from pathlib import Path

text = Path(sys.argv[1]).read_text()
start = text.index('static JNINativeMethod gMethods[]')
body = text[start:text.index('void register_java_lang_System', start)]
for kind, name, signature in re.findall(
        r'(NATIVE_METHOD|CRITICAL_NATIVE_METHOD)\(System,\s*([^,]+),\s*"([^"]+)"\)',
        body):
    print(f'{kind}\t{name}\t{signature}')
PY
art_methods="$stage/art-system-methods.tsv"
python3 - "$source_root/art/runtime/native/java_lang_System.cc" > "$art_methods" <<'PY'
import re
import sys
from pathlib import Path

text = Path(sys.argv[1]).read_text()
start = text.index('static JNINativeMethod gMethods[]')
body = text[start:text.index('void register_java_lang_System', start)]
for kind, name, signature in re.findall(
        r'(FAST_NATIVE_METHOD|NATIVE_METHOD|CRITICAL_NATIVE_METHOD)\(System,\s*([^,]+),\s*"([^"]+)"\)',
        body):
    print(f'{kind}\t{name}\t{signature}')
PY
java_methods="$stage/java-system-natives.tsv"
python3 - "$source_root/libcore/ojluni/src/main/java/java/lang/System.java" \
  > "$java_methods" <<'PY'
import re
import sys
from pathlib import Path

text = re.sub(r'/\*.*?\*/|//[^\n]*', '', Path(sys.argv[1]).read_text(), flags=re.S)
pattern = re.compile(
    r'\b(?:public|private|protected)\s+static\s+native\s+([\w\[\].]+)\s+(\w+)\s*\((.*?)\)\s*;',
    re.S)
descriptors = {
    'void': 'V', 'long': 'J', 'int': 'I', 'char': 'C', 'boolean': 'Z',
    'byte': 'B', 'short': 'S', 'float': 'F', 'double': 'D',
    'Object': 'Ljava/lang/Object;', 'String': 'Ljava/lang/String;',
    'Throwable': 'Ljava/lang/Throwable;',
    'InputStream': 'Ljava/io/InputStream;', 'PrintStream': 'Ljava/io/PrintStream;',
}
def descriptor(java_type):
    java_type = java_type.strip()
    if java_type.endswith('[]'):
        return '[' + descriptor(java_type[:-2])
    return descriptors[java_type]
for return_type, name, arguments in pattern.findall(text):
    parameters = ([descriptor(arg.strip().split()[0]) for arg in arguments.split(',')]
                  if arguments.strip() else [])
    print(f'{name}\t({"".join(parameters)}){descriptor(return_type)}')
PY

check_manifest() {
  local file="$1" expected_count="$2" expected_sha="$3" label="$4"
  local count digest
  count="$(wc -l < "$file" | tr -d ' ')"
  digest="$(sha256 "$file")"
  [[ "$count" == "$expected_count" && "$digest" == "$expected_sha" ]] ||
    fail "$label manifest drift count=$count sha=$digest"
}
check_manifest "$libcore_methods" "$LIBCORE_SYSTEM_METHOD_COUNT" \
  "$LIBCORE_SYSTEM_METHOD_MANIFEST_SHA256" libcore
check_manifest "$art_methods" "$ART_SYSTEM_METHOD_COUNT" \
  "$ART_SYSTEM_METHOD_MANIFEST_SHA256" ART
check_manifest "$java_methods" "$JAVA_SYSTEM_NATIVE_COUNT" \
  "$JAVA_SYSTEM_NATIVE_MANIFEST_SHA256" Java
native_bn_methods="$stage/native-bn-methods.tsv"
python3 - "$source_root/libcore/luni/src/main/native/libcore_math_NativeBN.cpp" \
  > "$native_bn_methods" <<'PY'
import re
import sys
from pathlib import Path

text = Path(sys.argv[1]).read_text()
start = text.index('static JNINativeMethod gMethods[]')
body = text[start:text.index('void register_libcore_math_NativeBN', start)]
for name, signature in re.findall(
        r'NATIVE_METHOD\(NativeBN,\s*([^,]+),\s*"([^"]+)"\)', body):
    print(f'{name}\t{signature}')
PY
check_manifest "$native_bn_methods" "$NATIVE_BN_METHOD_COUNT" \
  "$NATIVE_BN_METHOD_MANIFEST_SHA256" NativeBN
libcore_icu_methods="$stage/libcore-icu-methods.tsv"
python3 - "$source_root/libcore/luni/src/main/native/libcore_icu_ICU.cpp" \
  > "$libcore_icu_methods" <<'PY'
import re
import sys
from pathlib import Path

text = Path(sys.argv[1]).read_text()
start = text.index('static JNINativeMethod gMethods[]')
body = text[start:text.index('void register_libcore_icu_ICU', start)]
for name, signature in re.findall(
        r'NATIVE_METHOD\(ICU,\s*([^,]+),\s*"([^"]+)"\)', body):
    print(f'{name}\t{signature}')
PY
check_manifest "$libcore_icu_methods" "$LIBCORE_ICU_METHOD_COUNT" \
  "$LIBCORE_ICU_METHOD_MANIFEST_SHA256" libcore-ICU

cut -f2- "$libcore_methods" | sort -u > "$stage/libcore-owner.tsv"
cut -f2- "$art_methods" | sort -u > "$stage/art-owner.tsv"
sort -u "$java_methods" > "$stage/java-owner.tsv"
comm -12 "$stage/libcore-owner.tsv" "$stage/art-owner.tsv" > "$stage/owner-overlap.tsv"
[[ ! -s "$stage/owner-overlap.tsv" ]] || fail "ART/libcore System owner overlap"
cat "$stage/libcore-owner.tsv" "$stage/art-owner.tsv" | sort -u \
  > "$stage/owner-union.tsv"
cmp -s "$stage/owner-union.tsv" "$stage/java-owner.tsv" ||
  fail "ART+libcore System tables do not cover exact Java native declarations"
grep -Fx $'log\t(CLjava/lang/String;Ljava/lang/Throwable;)V' \
  "$stage/libcore-owner.tsv" >/dev/null || fail "System.log owner missing"

nativehelper_source="$project_root/_aosp/libnativehelper-full"
device_nativehelper="$project_root/_build/nativehelper-device-foundation/libnativehelper-device-darwin.a"
file_input_stream="$project_root/_build/file-input-stream-darwin/libopenjdk-file-input-stream-darwin.a"
openjdkjvm="$project_root/_build/openjdkjvm-darwin/libopenjdkjvm-darwin.a"
liblog_include="$project_root/_aosp/system/logging/liblog/include"
liblog="$project_root/_build/graphics-foundations/liblog-darwin.a"
for required in \
  "$device_nativehelper" "$file_input_stream" "$openjdkjvm" "$liblog" \
  "$nativehelper_source/include_jni/jni.h" \
  "$nativehelper_source/include/nativehelper/JNIHelp.h" \
  "$nativehelper_source/include_platform_header_only/nativehelper/jni_macros.h" \
  "$liblog_include/log/log.h" \
  "$project_root/probes/android16_system_natives_jni.c" \
  "$project_root/probes/system-natives/SystemNativesDarwinSmoke.java"; do
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
  -include "$patched_jvm_md"
  -Wall -Wextra -Werror -Wno-unused-parameter -Wno-unused-variable
  -Wno-sign-compare -Wno-deprecated-declarations
  -Wno-incompatible-pointer-types-discards-qualifiers
  -I"$patched_root/ojluni/src/main/native"
  -I"$native_root"
  -I"$nativehelper_source/include_jni"
  -I"$nativehelper_source/include"
  -I"$nativehelper_source/include_platform"
  -I"$nativehelper_source/include_platform_header_only"
  -I"$nativehelper_source/header_only_include"
  -I"$liblog_include"
  -I"$project_root/_aosp/external/zlib"
)

source_object="$objects/System.o"
"$cc" "${common_flags[@]}" -c "$patched_source" -o "$source_object"
[[ "$(file "$source_object")" == *"Mach-O 64-bit object arm64"* ]] ||
  fail "System object is not Darwin arm64"
native_bn_object="$objects/NativeBN.o"
jni_exception_object="$objects/JniException.o"
libcore_icu_object="$objects/libcore_icu_ICU.o"
jdk_internal_misc_vm_object="$objects/jdk_internal_misc_VM.o"
method_handle_object="$objects/java_lang_invoke_MethodHandle.o"
var_handle_object="$objects/java_lang_invoke_VarHandle.o"
native_bn_flags=(
  -std=c++17 -arch arm64 -isysroot "$sdk_root" -fPIC
  -ffunction-sections -fdata-sections -Wall -Wextra -Werror
  -I"$source_root/libcore/luni/src/main/native"
  -I"$boringssl_source/src/include"
  -I"$nativehelper_source/include_jni"
  -I"$nativehelper_source/include"
  -I"$nativehelper_source/include_platform"
  -I"$nativehelper_source/include_platform_header_only"
  -I"$nativehelper_source/header_only_include"
  -I"$liblog_include"
)
"$cxx" "${native_bn_flags[@]}" \
  -c "$source_root/libcore/luni/src/main/native/libcore_math_NativeBN.cpp" \
  -o "$native_bn_object"
"$cxx" "${native_bn_flags[@]}" \
  -c "$source_root/libcore/luni/src/main/native/JniException.cpp" \
  -o "$jni_exception_object"
"$cxx" "${native_bn_flags[@]}" -I"$native_root" \
  -c "$native_root/jdk_internal_misc_VM.cpp" \
  -o "$jdk_internal_misc_vm_object"
"$cxx" "${native_bn_flags[@]}" \
  -c "$source_root/libcore/luni/src/main/native/java_lang_invoke_MethodHandle.cpp" \
  -o "$method_handle_object"
"$cxx" "${native_bn_flags[@]}" \
  -c "$source_root/libcore/luni/src/main/native/java_lang_invoke_VarHandle.cpp" \
  -o "$var_handle_object"
"$cxx" "${native_bn_flags[@]}" \
  -I"$project_root/_aosp/external/icu-graphics/android_icu4c/include" \
  -I"$project_root/_aosp/external/icu-graphics/icu4c/source/common" \
  -I"$project_root/_aosp/external/icu-graphics/icu4c/source/i18n" \
  -c "$source_root/libcore/luni/src/main/native/libcore_icu_ICU.cpp" \
  -o "$libcore_icu_object"
archive="$stage/libopenjdk-system-natives-darwin.a"
"$libtool_bin" -static -o "$archive" "$source_object" \
  "$native_bn_object" "$jni_exception_object" "$libcore_icu_object" \
  "$jdk_internal_misc_vm_object" "$method_handle_object" \
  "$var_handle_object"
[[ "$(lipo -archs "$archive")" == arm64 ]] || fail "archive is not arm64"
[[ "$({ ar -t "$archive" || true; } | grep -v '^__\.SYMDEF' | wc -l | tr -d ' ')" == \
   "$LIBCORE_SYSTEM_SOURCE_COUNT" ]] || fail "archive member count mismatch"

symbols="$stage/archive-symbols.txt"
nm -aC "$archive" > "$symbols"
grep -E ' [Tt] _register_java_lang_System$' "$symbols" >/dev/null ||
  fail "libcore System registrar missing"
while IFS=$'\t' read -r kind method signature; do
  (: "$kind" "$signature")
  grep -E " [Tt] _System_${method}$" "$symbols" >/dev/null ||
    fail "libcore System definition missing: $method"
done < "$libcore_methods"
grep -E ' [Tt] register_libcore_math_NativeBN\(_JNIEnv\*\)$' "$symbols" >/dev/null ||
  fail "NativeBN registrar missing"
grep -E ' [Tt] register_libcore_icu_ICU\(_JNIEnv\*\)$' "$symbols" >/dev/null ||
  fail "libcore ICU registrar missing"
grep -E ' [Tt] register_jdk_internal_misc_VM\(_JNIEnv\*\)$' "$symbols" >/dev/null ||
  fail "jdk.internal.misc.VM registrar missing"
grep -E ' [Tt] register_java_lang_invoke_MethodHandle\(_JNIEnv\*\)$' "$symbols" >/dev/null ||
  fail "libcore MethodHandle registrar missing"
grep -E ' [Tt] register_java_lang_invoke_VarHandle\(_JNIEnv\*\)$' "$symbols" >/dev/null ||
  fail "libcore VarHandle registrar missing"

device_library="$stage/libsystem-natives-device-closure.dylib"
"$cxx" -arch arm64 -isysroot "$sdk_root" -dynamiclib \
  -Wl,-force_load,"$archive" "$file_input_stream" "$openjdkjvm" \
  "$device_nativehelper" "$liblog" "$boringssl_crypto" \
  "$project_root/_build/icu-foundation/libicui18n-darwin.a" \
  "$project_root/_build/icu-foundation/libicuuc-common-darwin.a" \
  "$project_root/_build/icu-foundation/libicuuc-stubdata-darwin.a" \
  -Wl,-exported_symbol,_register_java_lang_System \
  -Wl,-U,_darwin_art_bionic_fs_getcwd_core \
  -Wl,-dead_strip -framework CoreFoundation -o "$device_library"
device_undefined="$stage/device-retained-undefined.txt"
nm -u "$device_library" | sed 's/^[[:space:]]*//' | sort -u > "$device_undefined"
for closed in _JVM_CurrentTimeMillis _JNU_ThrowNullPointerException \
  _JNU_ThrowIllegalArgumentException _jniLogException \
  _jniRegisterNativeMethods __android_log_print; do
  if grep -Fx "$closed" "$device_undefined" >/dev/null; then
    fail "device provider closure retained undefined $closed"
  fi
done

managed_object="$objects/managed-probe.o"
"$cc" "${common_flags[@]}" \
  "-DDARWIN_ART_SYSTEM_SOURCE=\"$patched_source\"" \
  -c "$project_root/probes/android16_system_natives_jni.c" \
  -o "$managed_object"
managed_library="$stage/libsystem-natives-managed.dylib"
"$cxx" -arch arm64 -isysroot "$sdk_root" -dynamiclib \
  "$managed_object" "$file_input_stream" "$openjdkjvm" \
  "$device_nativehelper" "$liblog" \
  -Wl,-exported_symbol,_JNI_OnLoad -Wl,-dead_strip \
  -Wl,-U,_darwin_art_bionic_fs_getcwd_core \
  -framework CoreFoundation -o "$managed_library"
classes="$stage/classes"
mkdir -p "$classes"
javac --release 17 -encoding UTF-8 -d "$classes" \
  "$project_root/probes/system-natives/SystemNativesDarwinSmoke.java"
managed_output="$(java -cp "$classes" \
  dev.darwinart.probe.SystemNativesDarwinSmoke "$managed_library")"
[[ "$managed_output" == \
  'managed-system-natives: libcore=8 art=9 union=17 overlap=0 log=pass properties=pass streams=pass clock=pass map=pass' ]] ||
  fail "managed acceptance failed: $managed_output"

duplicates="$stage/duplicate-libcore-owners.txt"
: > "$duplicates"
for provider in \
  "$project_root/_build/runtime-bootstrap/libart-runtime-bootstrap-darwin.a" \
  "$project_root/_build/runtime-graphics-bootstrap/libart-runtime-graphics-bootstrap-darwin.a" \
  "$project_root/_build/libcore-darwin-linux/libcore-darwin-linux.a"; do
  [[ -f "$provider" ]] || continue
  if nm -aC "$provider" | grep -E \
    ' [TDSBC] _(System_(mapLibraryName|setErr0|setOut0|setIn0)|register_java_lang_System)$' \
    >> "$duplicates"; then
    fail "duplicate libcore System owner in $provider"
  fi
done

atomic="$stage/atomic-registration.txt"
cat > "$atomic" <<'EOF'
ART-owner=9 arraycopy methods; remains registered by Runtime::RegisterRuntimeNativeMethods
libcore-owner=8 disjoint methods; call global register_java_lang_System exactly once after ART start
union=17 exact java.lang.System native declarations; overlap=0
integration-requires=remove compat system_methods[3] and direct Register(java/lang/System,...,3)
integration-forbids=partial currentTimeMillis/nanoTime/specialProperties owner mixed with full libcore table
link-order=consumer-bootstrap,libopenjdk-system-natives-darwin.a,libopenjdk-file-input-stream-darwin.a,libopenjdkjvm-darwin.a,libnativehelper-device-darwin.a,liblog-darwin.a
EOF

undefined="$stage/archive-undefined.txt"
nm -u "$archive" | sed 's/^[[:space:]]*//' | sort -u > "$undefined"
mkdir -p "$build_dir"
cp "$archive" "$build_dir/libopenjdk-system-natives-darwin.a"
cp "$boringssl_crypto" "$build_dir/libcrypto-boringssl-darwin.a"
cp "$native_bn_methods" "$build_dir/native-bn-methods.tsv"
cp "$libcore_icu_methods" "$build_dir/libcore-icu-methods.tsv"
cp "$libcore_methods" "$build_dir/libcore-system-methods.tsv"
cp "$art_methods" "$build_dir/art-system-methods.tsv"
cp "$java_methods" "$build_dir/java-system-natives.tsv"
cp "$stage/owner-union.tsv" "$build_dir/system-owner-union.tsv"
cp "$stage/owner-overlap.tsv" "$build_dir/system-owner-overlap.tsv"
cp "$symbols" "$build_dir/archive-symbols.txt"
cp "$undefined" "$build_dir/archive-undefined.txt"
cp "$device_undefined" "$build_dir/device-retained-undefined.txt"
cp "$duplicates" "$build_dir/duplicate-libcore-owners.txt"
cp "$atomic" "$build_dir/atomic-registration.txt"
cp "$patched_source" "$build_dir/System.darwin.c"

echo "system-natives: libcore=8 art=9 java=17 NativeBN=7 ICU=10 union=17 overlap=0 log=pass managed=pass duplicate-libcore-owners=0 archive=Mach-O-arm64"
