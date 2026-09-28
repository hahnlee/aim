#!/bin/bash
set -euo pipefail
export LC_ALL=C

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/.." && pwd)"
lock_file="$project_root/upstream/android16-unix-native-dispatcher-darwin.lock"
source_root="$project_root/_aosp/libcore-unix-native-dispatcher"
native_root="$source_root/ojluni/src/main/native"
build_dir="$project_root/_build/unix-native-dispatcher-darwin"
patch_file="$project_root/patches/libcore-openjdk/0003-darwin-unix-native-dispatcher-times.patch"
unix_process_patch="$project_root/patches/libcore-openjdk/0004-darwin-unix-process.patch"
strict_math_patch="$project_root/patches/libcore-openjdk/0005-darwin-strict-math-fdlibm-include.patch"

# shellcheck disable=SC1090
source "$lock_file"

sha256() { shasum -a 256 "$1" | awk '{print $1}'; }
fail() { echo "unix-native-dispatcher: $*" >&2; exit 3; }

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
materialize ojluni/src/main/native/UnixNativeDispatcher.c \
  "$UNIX_NATIVE_DISPATCHER_C_SHA256"
materialize ojluni/src/main/native/sun_nio_fs_UnixNativeDispatcher.h \
  "$UNIX_NATIVE_DISPATCHER_H_SHA256"
materialize ojluni/src/main/java/sun/nio/fs/UnixNativeDispatcher.java \
  "$UNIX_NATIVE_DISPATCHER_JAVA_SHA256"
materialize ojluni/src/main/native/jni_util.c "$JNI_UTIL_C_SHA256"
materialize ojluni/src/main/native/jni_util_md.c "$JNI_UTIL_MD_C_SHA256"
materialize ojluni/src/main/native/jni_util.h "$JNI_UTIL_H_SHA256"
materialize ojluni/src/main/native/jlong.h "$JLONG_H_SHA256"
materialize ojluni/src/main/native/jlong_md.h "$JLONG_MD_H_SHA256"
materialize ojluni/src/main/native/jvm.h "$JVM_H_SHA256"
materialize ojluni/src/main/native/jvm_md.h "$JVM_MD_H_SHA256"
materialize ojluni/src/main/native/io_util.h "$IO_UTIL_H_SHA256"
materialize ojluni/src/main/native/classfile_constants.h \
  "$CLASSFILE_CONSTANTS_H_SHA256"
materialize ojluni/src/main/native/UNIXProcess_md.c "$UNIX_PROCESS_C_SHA256"
materialize ojluni/src/main/native/StrictMath.c "$STRICT_MATH_C_SHA256"

[[ "$(sha256 "$patch_file")" == "$DARWIN_PATCH_SHA256" ]] ||
  fail "Darwin timestamp patch checksum mismatch"
[[ "$(sha256 "$unix_process_patch")" == "$DARWIN_UNIX_PROCESS_PATCH_SHA256" ]] ||
  fail "Darwin UNIXProcess patch checksum mismatch"
[[ "$(sha256 "$strict_math_patch")" == "$DARWIN_STRICT_MATH_PATCH_SHA256" ]] ||
  fail "Darwin StrictMath patch checksum mismatch"
stage="$(mktemp -d "${TMPDIR:-/tmp}/aim-und.XXXXXX")"
trap 'rm -rf "$stage"' EXIT
patched_root="$stage/source"
mkdir -p "$patched_root/ojluni/src/main/native"
cp "$native_root/UnixNativeDispatcher.c" \
  "$patched_root/ojluni/src/main/native/UnixNativeDispatcher.c"
cp "$native_root/UNIXProcess_md.c" \
  "$patched_root/ojluni/src/main/native/UNIXProcess_md.c"
cp "$native_root/StrictMath.c" \
  "$patched_root/ojluni/src/main/native/StrictMath.c"
patch --batch --forward -p1 -d "$patched_root" < "$patch_file" >/dev/null
patch --batch --forward -p1 -d "$patched_root" < "$unix_process_patch" >/dev/null
patch --batch --forward -p1 -d "$patched_root" < "$strict_math_patch" >/dev/null
patched_source="$patched_root/ojluni/src/main/native/UnixNativeDispatcher.c"
patched_unix_process="$patched_root/ojluni/src/main/native/UNIXProcess_md.c"
patched_strict_math="$patched_root/ojluni/src/main/native/StrictMath.c"
grep -F '#include <fdlibm.h>' "$patched_strict_math" >/dev/null ||
  fail "patched StrictMath fdlibm include missing"
[[ "$(sha256 "$patched_source")" == "$PATCHED_UNIX_NATIVE_DISPATCHER_C_SHA256" ]] ||
  fail "patched source checksum mismatch"
for expected in st_atimespec.tv_nsec st_mtimespec.tv_nsec st_ctimespec.tv_nsec; do
  grep -F "$expected" "$patched_source" >/dev/null ||
    fail "Darwin nanosecond mapping missing: $expected"
done

python3 - "$native_root/Android.bp" <<'PY'
import re
import sys
from pathlib import Path

text = Path(sys.argv[1]).read_text()
start = text.index('name: "libopenjdk_native_srcs"')
body_start = text.index('srcs: [', start)
body_end = text.index('],', body_start)
sources = re.findall(r'"([^" ]+\.(?:c|cpp))"', text[body_start:body_end])
if len(sources) != 59 or 'UnixNativeDispatcher.c' not in sources:
    raise SystemExit(f'libopenjdk source membership drift: count={len(sources)}')
PY

methods="$stage/unix-native-dispatcher-methods.tsv"
python3 - "$native_root/UnixNativeDispatcher.c" > "$methods" <<'PY'
import re
import sys
from pathlib import Path

text = Path(sys.argv[1]).read_text()
start = text.index('static JNINativeMethod gMethods[]')
body = text[start:text.index(
    'void register_java_sun_nio_fs_UnixNativeDispatcher', start)]
for name, signature in re.findall(
        r'NATIVE_METHOD\(Java_sun_nio_fs_UnixNativeDispatcher,\s*([^,]+),\s*"([^"]+)"\)',
        body):
    print(f'{name}\t{signature}')
PY
method_count="$(wc -l < "$methods" | tr -d ' ')"
method_sha="$(sha256 "$methods")"
[[ "$method_count" == "$UNIX_NATIVE_DISPATCHER_METHOD_COUNT" &&
   "$method_sha" == "$UNIX_NATIVE_DISPATCHER_METHOD_MANIFEST_SHA256" ]] ||
  fail "method table drift count=$method_count sha=$method_sha"
for required_method in $'init\t()I' $'open0\t(JII)I' \
  $'stat0\t(JLsun/nio/fs/UnixFileAttributes;)V' $'readdir\t(J)[B'; do
  grep -Fx "$required_method" "$methods" >/dev/null ||
    fail "representative native missing: $required_method"
done
grep -F 'register_java_sun_nio_fs_UnixNativeDispatcher(env);' \
  "$native_root/OnLoad.cpp" >/dev/null || fail "canonical OnLoad call missing"
grep -F 'register_java_lang_UNIXProcess(env);' \
  "$native_root/OnLoad.cpp" >/dev/null || fail "canonical UNIXProcess OnLoad call missing"

unix_process_methods="$stage/unix-process-methods.tsv"
python3 - "$native_root/UNIXProcess_md.c" > "$unix_process_methods" <<'PY'
import re
import sys
from pathlib import Path

text = Path(sys.argv[1]).read_text()
start = text.index('static JNINativeMethod gMethods[]')
body = text[start:text.index('void register_java_lang_UNIXProcess', start)]
for name, signature in re.findall(
        r'NATIVE_METHOD\(UNIXProcess,\s*([^,]+),\s*"([^"]+)"\)', body):
    print(f'{name}\t{signature}')
PY
unix_process_method_count="$(wc -l < "$unix_process_methods" | tr -d ' ')"
unix_process_method_sha="$(sha256 "$unix_process_methods")"
[[ "$unix_process_method_count" == "$UNIX_PROCESS_METHOD_COUNT" &&
   "$unix_process_method_sha" == "$UNIX_PROCESS_METHOD_MANIFEST_SHA256" ]] ||
  fail "UNIXProcess method table drift count=$unix_process_method_count sha=$unix_process_method_sha"

strict_math_methods="$stage/strict-math-methods.tsv"
python3 - "$native_root/StrictMath.c" > "$strict_math_methods" <<'PY'
import re
import sys
from pathlib import Path

text = Path(sys.argv[1]).read_text()
start = text.index('static JNINativeMethod gMethods[]')
body = text[start:text.index('void register_java_lang_StrictMath', start)]
for name, signature in re.findall(
        r'NATIVE_METHOD\(StrictMath,\s*([^,]+),\s*"([^"]+)"\)', body):
    print(f'{name}\t{signature}')
PY
strict_math_method_count="$(wc -l < "$strict_math_methods" | tr -d ' ')"
strict_math_method_sha="$(sha256 "$strict_math_methods")"
[[ "$strict_math_method_count" == "$STRICT_MATH_METHOD_COUNT" &&
   "$strict_math_method_sha" == "$STRICT_MATH_METHOD_MANIFEST_SHA256" ]] ||
  fail "StrictMath method table drift count=$strict_math_method_count sha=$strict_math_method_sha"

fdlibm_source="$project_root/_aosp/fdlibm-full"
if [[ ! -d "$fdlibm_source/.git" ]]; then
  git clone --filter=blob:none --no-checkout \
    https://android.googlesource.com/platform/external/fdlibm "$fdlibm_source"
fi
if ! git -C "$fdlibm_source" cat-file -e "$FDLIBM_REVISION^{commit}" 2>/dev/null; then
  git -C "$fdlibm_source" fetch --depth=1 origin "$FDLIBM_REVISION"
fi
git -C "$fdlibm_source" checkout --detach "$FDLIBM_REVISION" >/dev/null
[[ "$(git -C "$fdlibm_source" rev-parse HEAD)" == "$FDLIBM_REVISION" ]] ||
  fail "fdlibm checkout revision drift"
[[ "$(sha256 "$fdlibm_source/Android.bp")" == "$FDLIBM_ANDROID_BP_SHA256" ]] ||
  fail "fdlibm Android.bp drift"
fdlibm_sources="$stage/fdlibm-sources.txt"
python3 - "$fdlibm_source/Android.bp" > "$fdlibm_sources" <<'PY'
import re
import sys
from pathlib import Path

text = Path(sys.argv[1]).read_text()
start = text.index('srcs: [')
end = text.index('],', start)
for source in re.findall(r'"([^"]+\.c)"', text[start:end]):
    print(source)
PY
[[ "$(wc -l < "$fdlibm_sources" | tr -d ' ')" == "$FDLIBM_SOURCE_COUNT" ]] ||
  fail "fdlibm source membership drift"

# Darwin intentionally does not advertise the atomic openat group while
# futimesat is absent. Individual entrypoints remain the complete upstream
# table and fail through the existing capability/internal-error contract;
# none are replaced with success stubs.
grep -F 'my_futimesat_func = (futimesat_func*) dlsym(RTLD_DEFAULT, "futimesat")' \
  "$native_root/UnixNativeDispatcher.c" >/dev/null ||
  fail "openat capability dependency drift"
grep -F 'capabilities |= sun_nio_fs_UnixNativeDispatcher_SUPPORTS_FUTIMES' \
  "$native_root/UnixNativeDispatcher.c" >/dev/null ||
  fail "Darwin futimes capability drift"

nativehelper_source="$project_root/_aosp/libnativehelper-full"
device_nativehelper="$project_root/_build/nativehelper-device-foundation/libnativehelper-device-darwin.a"
file_input_stream="$project_root/_build/file-input-stream-darwin/libopenjdk-file-input-stream-darwin.a"
liblog_include="$project_root/_aosp/system/logging/liblog/include"
liblog="$project_root/_build/graphics-foundations/liblog-darwin.a"
for required in \
  "$device_nativehelper" "$file_input_stream" "$liblog" \
  "$nativehelper_source/include_jni/jni.h" \
  "$nativehelper_source/include/nativehelper/JNIHelp.h" \
  "$liblog_include/android/log.h" \
  "$project_root/compat/darwin_openjdk_nio_fs_redirect.h" \
  "$project_root/tools/bionic-errno-tls/include/aim_bionic_errno.h" \
  "$project_root/tools/bionic-fs-facade/include/aim_bionic_fs.h" \
  "$project_root/probes/android16_unix_native_dispatcher_jni.c" \
  "$project_root/probes/unix-native-dispatcher/UnixNativeDispatcherDarwinSmoke.java" \
  "$project_root/compat/darwin_openjdk_nio_copy.c" \
  "$project_root/_aosp/libcore-full/ojluni/src/main/native/sun_nio_fs_UnixCopyFile.h"; do
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
  -Wno-sign-compare -Wno-deprecated-declarations
  -Wno-incompatible-pointer-types-discards-qualifiers
  -I"$native_root"
  -I"$nativehelper_source/include_jni"
  -I"$nativehelper_source/include"
  -I"$nativehelper_source/include_platform"
  -I"$nativehelper_source/include_platform_header_only"
  -I"$nativehelper_source/header_only_include"
  -I"$liblog_include"
  -I"$project_root/tools/bionic-errno-tls/include"
  -I"$project_root/tools/bionic-fs-facade/include"
  -I"$project_root/tools/bionic-ioctl-facade/include"
  -I"$project_root/tools/bionic-socket-broker-adapter/include"
  -I"$project_root/_aosp/libcore-full/ojluni/src/main/native"
)

source_object="$objects/UnixNativeDispatcher.o"
"$cc" "${common_flags[@]}" \
  -include "$project_root/compat/darwin_openjdk_nio_fs_redirect.h" \
  -c "$patched_source" -o "$source_object"
[[ "$(file "$source_object")" == *"Mach-O 64-bit object arm64"* ]] ||
  fail "UnixNativeDispatcher object is not Darwin arm64"
unix_process_object="$objects/UNIXProcess.o"
"$cc" "${common_flags[@]}" -c "$patched_unix_process" \
  -o "$unix_process_object"
[[ "$(file "$unix_process_object")" == *"Mach-O 64-bit object arm64"* ]] ||
  fail "UNIXProcess object is not Darwin arm64"
strict_math_object="$objects/StrictMath.o"
# Put the pinned fdlibm include directory first: common_flags contains the
# libcore-full native directory, which also has an unrelated fdlibm.h.
"$cc" -I"$fdlibm_source" "${common_flags[@]}" \
  -c "$patched_strict_math" -o "$strict_math_object"
[[ "$(file "$strict_math_object")" == *"Mach-O 64-bit object arm64"* ]] ||
  fail "StrictMath object is not Darwin arm64"
fdlibm_objects=()
while IFS= read -r source; do
  object="$objects/fdlibm-${source%.c}.o"
  "$cc" -std=c99 -arch arm64 -isysroot "$sdk_root" -fPIC \
    -D_IEEE_LIBM -D__LITTLE_ENDIAN -fno-strict-aliasing \
    -Werror -Wno-sign-compare -Wno-dangling-else \
    -Wno-unknown-pragmas -Wno-logical-op-parentheses \
    -Wno-sometimes-uninitialized -I"$fdlibm_source" \
    -c "$fdlibm_source/$source" -o "$object"
  fdlibm_objects+=("$object")
done < "$fdlibm_sources"
fdlibm_archive="$stage/libfdlibm-darwin.a"
"$libtool_bin" -static -o "$fdlibm_archive" "${fdlibm_objects[@]}"
[[ "$(lipo -archs "$fdlibm_archive")" == arm64 ]] ||
  fail "fdlibm archive is not Darwin arm64"
host_source_object="$objects/UnixNativeDispatcher-host-probe.o"
"$cc" "${common_flags[@]}" -c "$patched_source" -o "$host_source_object"
copy_object="$objects/darwin_openjdk_nio_copy.o"
"$cc" "${common_flags[@]}" \
  -include "$project_root/compat/darwin_openjdk_nio_fs_redirect.h" \
  -c "$project_root/compat/darwin_openjdk_nio_copy.c" -o "$copy_object"
archive="$stage/libopenjdk-unix-native-dispatcher-darwin.a"
"$libtool_bin" -static -o "$archive" "$source_object" "$unix_process_object" \
  "$strict_math_object" "$copy_object"
[[ "$(lipo -archs "$archive")" == arm64 ]] || fail "archive is not arm64"
member_count="$({ ar -t "$archive" || true; } | grep -v '^__\.SYMDEF' | wc -l | tr -d ' ')"
expected_source_count="$((UNIX_NATIVE_DISPATCHER_SOURCE_COUNT + UNIX_PROCESS_SOURCE_COUNT + STRICT_MATH_SOURCE_COUNT + 1))"
[[ "$member_count" == "$expected_source_count" ]] ||
  fail "archive member count=$member_count"

symbols="$stage/archive-symbols.txt"
nm -aC "$archive" > "$symbols"
grep -E ' [TDSBC] _register_java_sun_nio_fs_UnixNativeDispatcher$' \
  "$symbols" >/dev/null || fail "complete registrar missing"
grep -E ' [TDSBC] _register_java_lang_UNIXProcess$' \
  "$symbols" >/dev/null || fail "UNIXProcess registrar missing"
grep -E ' [TDSBC] _register_java_lang_StrictMath$' \
  "$symbols" >/dev/null || fail "StrictMath registrar missing"
while IFS=$'\t' read -r method signature; do
  (void_signature="$signature"; : "$void_signature")
  grep -E " [TDSBC] _Java_sun_nio_fs_UnixNativeDispatcher_${method}$" \
    "$symbols" >/dev/null || fail "native definition missing: $method"
done < "$methods"
grep -E ' [TDSBC] _Java_sun_nio_fs_UnixCopyFile_transfer$' "$symbols" >/dev/null ||
  fail "UnixCopyFile.transfer native definition missing"
while IFS=$'\t' read -r method signature; do
  (void_signature="$signature"; : "$void_signature")
  grep -E " [TDSBC] _UNIXProcess_${method}$" "$symbols" >/dev/null ||
    fail "UNIXProcess native definition missing: $method"
done < "$unix_process_methods"

device_library="$stage/libunix-native-dispatcher-device-closure.dylib"
"$cxx" -arch arm64 -isysroot "$sdk_root" -dynamiclib \
  "$host_source_object" "$copy_object" "$file_input_stream" \
  "$device_nativehelper" "$liblog" \
  -Wl,-exported_symbol,_register_java_sun_nio_fs_UnixNativeDispatcher \
  -Wl,-dead_strip -framework CoreFoundation -o "$device_library"
device_undefined="$stage/device-retained-undefined.txt"
nm -u "$device_library" | sed 's/^[[:space:]]*//' | sort -u > "$device_undefined"
for closed in _JNU_NewObjectByName _JNU_ThrowInternalError \
  _JNU_ThrowOutOfMemoryError _jniRegisterNativeMethods; do
  if grep -Fx "$closed" "$device_undefined" >/dev/null; then
    fail "device provider closure retained undefined $closed"
  fi
done

managed_probe="$objects/managed-probe.o"
"$cc" "${common_flags[@]}" \
  -c "$project_root/probes/android16_unix_native_dispatcher_jni.c" \
  -o "$managed_probe"
managed_library="$stage/libunix-native-dispatcher-managed.dylib"
"$cxx" -arch arm64 -isysroot "$sdk_root" -dynamiclib \
  "$managed_probe" "$host_source_object" "$copy_object" "$file_input_stream" \
  -Wl,-exported_symbol,_JNI_OnLoad -Wl,-dead_strip -o "$managed_library"
classes="$stage/classes"
mkdir -p "$classes"
javac --release 17 -encoding UTF-8 -d "$classes" \
  "$project_root/probes/unix-native-dispatcher/UnixNativeDispatcherDarwinSmoke.java"
managed_output="$(java -cp "$classes" \
  dev.aim.probe.UnixNativeDispatcherDarwinSmoke "$managed_library")"
[[ "$managed_output" == \
  'managed-unix-native-dispatcher: methods=47 init=pass posix-io=pass directory=pass links=pass identity=pass capabilities=65540' ]] ||
  fail "managed acceptance failed: $managed_output"

duplicates="$stage/duplicate-full-owners.txt"
: > "$duplicates"
for provider in \
  "$project_root/_build/runtime-bootstrap/libart-runtime-bootstrap-darwin.a" \
  "$project_root/_build/runtime-graphics-bootstrap/libart-runtime-graphics-bootstrap-darwin.a" \
  "$project_root/_build/libcore-darwin-linux/libcore-darwin-linux.a"; do
  [[ -f "$provider" ]] || continue
  if nm -aC "$provider" | grep -E \
    ' [TDSBC] _register_java_sun_nio_fs_UnixNativeDispatcher$' \
    >> "$duplicates"; then
    fail "duplicate complete UnixNativeDispatcher owner in $provider"
  fi
done

policy="$stage/darwin-capability-policy.txt"
cat > "$policy" <<'EOF'
complete-table=47 UnixNativeDispatcher + 4 UNIXProcess + 20 StrictMath source-derived methods; no dropped or replacement stubs
strict-math=Android 16 StrictMath.c backed by pinned 80-source fdlibm; no host-libm substitution
supports-futimes=yes; Darwin futimes(2) is used directly
supports-birthtime=yes; st_birthtime and Darwin timespec nanoseconds are mapped
supports-openat-group=no while futimesat is absent; capability bit remains clear
privileged-operations=mknod/chown use real Darwin syscalls and propagate UnixException(errno)
EOF
atomic="$stage/atomic-registration.txt"
cat > "$atomic" <<'EOF'
registration=call register_java_sun_nio_fs_UnixNativeDispatcher exactly once
registration-order=after Float,Double,System and existing java.io/NIO owners; before application code
integration-forbids=partial RegisterNatives table or mixed method ownership
link-order=consumer-bootstrap,libopenjdk-unix-native-dispatcher-darwin.a,libopenjdk-file-input-stream-darwin.a,libnativehelper-device-darwin.a,liblog-darwin.a
EOF

undefined="$stage/archive-undefined.txt"
nm -u "$archive" | sed 's/^[[:space:]]*//' | sort -u > "$undefined"
for redirected in _aim_bionic_open _aim_bionic_mkdir \
  _aim_bionic_stat _aim_bionic_write; do
  grep -Fx "$redirected" "$undefined" >/dev/null ||
    fail "production NIO redirect missing $redirected"
done
mkdir -p "$build_dir"
cp "$archive" "$build_dir/libopenjdk-unix-native-dispatcher-darwin.a"
cp "$fdlibm_archive" "$build_dir/libfdlibm-darwin.a"
cp "$methods" "$build_dir/unix-native-dispatcher-methods.tsv"
cp "$unix_process_methods" "$build_dir/unix-process-methods.tsv"
cp "$strict_math_methods" "$build_dir/strict-math-methods.tsv"
cp "$symbols" "$build_dir/archive-symbols.txt"
cp "$undefined" "$build_dir/archive-undefined.txt"
cp "$device_undefined" "$build_dir/device-retained-undefined.txt"
cp "$duplicates" "$build_dir/duplicate-full-owners.txt"
cp "$policy" "$build_dir/darwin-capability-policy.txt"
cp "$atomic" "$build_dir/atomic-registration.txt"
cp "$patched_source" "$build_dir/UnixNativeDispatcher.darwin.c"

echo "unix-native-dispatcher: sources=3/59 methods=$method_count+4+20 fdlibm=80 capabilities=futimes+birthtime/openat-group-disabled managed=pass duplicate-full-owners=0 archive=Mach-O-arm64"
