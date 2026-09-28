#!/bin/bash
set -euo pipefail
export LC_ALL=C

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/.." && pwd)"
lock_file="$project_root/upstream/android16-openjdk-nio-mapping.lock"
source_root="$project_root/_aosp/libcore-openjdk-nio-mapping"
native_root="$source_root/ojluni/src/main/native"
build_dir="$project_root/_build/openjdk-nio-mapping"

# shellcheck disable=SC1090
source "$lock_file"

sha256() { shasum -a 256 "$1" | awk '{print $1}'; }
fail() { echo "openjdk-nio-mapping: $*" >&2; exit 3; }
blocked() { echo "openjdk-nio-mapping: BLOCKED: $*" >&2; exit 2; }

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

materialize ojluni/src/main/native/Android.bp "$ANDROID_BP_SHA256"
materialize ojluni/src/main/native/OnLoad.cpp "$ONLOAD_CPP_SHA256"
materialize ojluni/src/main/native/FileChannelImpl.c \
  "$FILECHANNELIMPL_C_SHA256"
materialize ojluni/src/main/native/FileDispatcherImpl.c \
  "$FILEDISPATCHERIMPL_C_SHA256"
materialize ojluni/src/main/native/NativeThread.c "$NATIVETHREAD_C_SHA256"
materialize ojluni/src/main/java/sun/nio/ch/FileChannelImpl.java \
  "$FILECHANNELIMPL_JAVA_SHA256"
materialize ojluni/src/main/java/sun/nio/ch/FileDispatcherImpl.java \
  "$FILEDISPATCHERIMPL_JAVA_SHA256"
materialize ojluni/src/main/java/sun/nio/ch/NativeThread.java \
  "$NATIVETHREAD_JAVA_SHA256"
materialize ojluni/src/main/native/IOUtil.c "$IOUTIL_C_SHA256"
materialize ojluni/src/main/native/jni_util.c "$JNI_UTIL_C_SHA256"
materialize ojluni/src/main/native/jni_util_md.c "$JNI_UTIL_MD_C_SHA256"
materialize ojluni/src/main/native/jni_util.h "$JNI_UTIL_H_SHA256"
materialize ojluni/src/main/native/classfile_constants.h \
  "$CLASSFILE_CONSTANTS_H_SHA256"
materialize ojluni/src/main/native/jvm.h "$JVM_H_SHA256"
materialize ojluni/src/main/native/jvm_md.h "$JVM_MD_H_SHA256"
materialize ojluni/src/main/native/jlong.h "$JLONG_H_SHA256"
materialize ojluni/src/main/native/jlong_md.h "$JLONG_MD_H_SHA256"
materialize ojluni/src/main/native/nio.h "$NIO_H_SHA256"
materialize ojluni/src/main/native/nio_util.h "$NIO_UTIL_H_SHA256"
materialize ojluni/src/main/native/java_lang_Long.h "$JAVA_LANG_LONG_H_SHA256"
materialize ojluni/src/main/native/java_lang_Integer.h \
  "$JAVA_LANG_INTEGER_H_SHA256"
materialize ojluni/src/main/native/sun_nio_ch_FileChannelImpl.h \
  "$SUN_NIO_CH_FILECHANNELIMPL_H_SHA256"
materialize ojluni/src/main/native/sun_nio_ch_FileDispatcherImpl.h \
  "$SUN_NIO_CH_FILEDISPATCHERIMPL_H_SHA256"
materialize ojluni/src/main/native/sun_nio_ch_IOStatus.h \
  "$SUN_NIO_CH_IOSTATUS_H_SHA256"

stage="$(mktemp -d "${TMPDIR:-/tmp}/aim-openjdk-nio.XXXXXX")"
trap 'rm -rf "$stage"' EXIT

native_thread_patch="$project_root/patches/libcore-openjdk/0003-darwin-native-thread-signal-lifecycle.patch"
[[ -f "$native_thread_patch" ]] || blocked "missing NativeThread lifecycle patch"
[[ "$(sha256 "$native_thread_patch")" == "$NATIVETHREAD_LIFECYCLE_PATCH_SHA256" ]] ||
  fail "NativeThread lifecycle patch checksum mismatch"
patched_native_root="$stage/patched/ojluni/src/main/native"
mkdir -p "$patched_native_root"
cp "$native_root/NativeThread.c" "$patched_native_root/NativeThread.c"
(cd "$stage/patched" && git init -q &&
  git apply --recount "$native_thread_patch")
[[ "$(sha256 "$patched_native_root/NativeThread.c")" == "$PATCHED_NATIVETHREAD_C_SHA256" ]] ||
  fail "patched NativeThread source checksum mismatch"

printf '%s\n' FileChannelImpl.c FileDispatcherImpl.c NativeThread.c \
  > "$stage/target-sources.txt"
[[ "$(wc -l < "$stage/target-sources.txt" | tr -d ' ')" == \
   "$TARGET_SOURCE_COUNT" && \
   "$(sha256 "$stage/target-sources.txt")" == \
   "$TARGET_SOURCE_MANIFEST_SHA256" ]] || fail "target source manifest drift"

python3 - "$native_root/Android.bp" "$stage/target-sources.txt" <<'PY'
import re
import sys
from pathlib import Path

bp = Path(sys.argv[1]).read_text()
start = bp.index('name: "libopenjdk_native_srcs"')
body = bp[bp.index('srcs: [', start):bp.index('],', bp.index('srcs: [', start))]
selected = set(re.findall(r'"([^" ]+\.(?:c|cpp))"', body))
targets = Path(sys.argv[2]).read_text().splitlines()
missing = [source for source in targets if source not in selected]
if missing:
    raise SystemExit(f'target source left libopenjdk_native_srcs: {missing}')
PY

python3 - "$native_root" > "$stage/methods.tsv" <<'PY'
import re
import sys
from pathlib import Path

root = Path(sys.argv[1])
for source in ('FileChannelImpl.c', 'FileDispatcherImpl.c', 'NativeThread.c'):
    text = (root / source).read_text()
    start = text.index('static JNINativeMethod gMethods[]')
    body = text[start:text.index('};', start)]
    for name, signature in re.findall(
            r'NATIVE_METHOD\([^,]+,\s*([^,]+),\s*"([^"]+)"\)', body):
        print(f'{source}\t{name}\t{signature}')
PY
[[ "$(sha256 "$stage/methods.tsv")" == "$METHOD_MANIFEST_SHA256" ]] ||
  fail "native method manifest drift"
[[ "$(awk -F '\t' '$1 == "FileChannelImpl.c" {n++} END {print n+0}' \
      "$stage/methods.tsv")" == "$FILECHANNEL_METHOD_COUNT" ]] ||
  fail "FileChannelImpl method count drift"
[[ "$(awk -F '\t' '$1 == "FileDispatcherImpl.c" {n++} END {print n+0}' \
      "$stage/methods.tsv")" == "$FILEDISPATCHER_METHOD_COUNT" ]] ||
  fail "FileDispatcherImpl method count drift"
[[ "$(awk -F '\t' '$1 == "NativeThread.c" {n++} END {print n+0}' \
      "$stage/methods.tsv")" == "$NATIVETHREAD_METHOD_COUNT" ]] ||
  fail "NativeThread method count drift"

grep -F 's = nd.size(fd);' \
  "$source_root/ojluni/src/main/java/sun/nio/ch/FileChannelImpl.java" \
  >/dev/null || fail "FileChannel.size no longer delegates to dispatcher size"
grep -F 'static native long size0(FileDescriptor fd) throws IOException;' \
  "$source_root/ojluni/src/main/java/sun/nio/ch/FileDispatcherImpl.java" \
  >/dev/null || fail "FileDispatcher.size0 managed contract drift"
grep -F 'static native long current();' \
  "$source_root/ojluni/src/main/java/sun/nio/ch/NativeThread.java" \
  >/dev/null || fail "NativeThread.current managed contract drift"

python3 - "$stage/methods.tsv" \
  "$project_root/probes/android16_openjdk_nio_mapping_jni.c" <<'PY'
import re
import sys
from pathlib import Path

manifest = [line.rstrip('\n').split('\t') for line in Path(sys.argv[1]).open()]
probe = Path(sys.argv[2]).read_text()
for source, array in (
        ('FileChannelImpl.c', 'kFileChannelMethods'),
        ('FileDispatcherImpl.c', 'kFileDispatcherMethods')):
    start = probe.index(f'static JNINativeMethod {array}[]')
    body = probe[start:probe.index('};', start)]
    actual = re.findall(r'METHOD\("([^"]+)",\s*"([^"]+)"', body)
    expected = [(name, signature) for owner, name, signature in manifest
                if owner == source]
    if actual != expected:
        raise SystemExit(f'acceptance table differs from upstream {source}')
PY

python3 - "$native_root/OnLoad.cpp" <<'PY'
import sys
from pathlib import Path

text = Path(sys.argv[1]).read_text()
calls = [
    'register_sun_nio_ch_IOUtil(env);',
    'register_sun_nio_ch_SocketChannelImpl(env);',
    'register_sun_nio_ch_FileChannelImpl(env);',
    'register_sun_nio_ch_FileDispatcherImpl(env);',
    'register_java_io_FileInputStream(env);',
    'register_java_util_prefs_FileSystemPreferences(env);',
    'register_sun_nio_ch_NativeThread(env);',
]
positions = [text.index(call) for call in calls]
if positions != sorted(positions):
    raise SystemExit('OpenJDK NIO registration order drift')
PY

nativehelper_source="$project_root/_aosp/libnativehelper-full"
nativehelper_archive="$project_root/_build/nativehelper-foundation/libnativehelper_jvm.a"
liblog_include="$project_root/_aosp/system/logging/liblog/include"
liblog_archive="$project_root/_build/graphics-foundations/liblog-darwin.a"
for required in \
  "$nativehelper_source/include_jni/jni.h" \
  "$nativehelper_source/include/nativehelper/JNIHelp.h" \
  "$nativehelper_archive" "$liblog_include/android/log.h" "$liblog_archive" \
  "$project_root/probes/android16_openjdk_nio_mapping_jni.c" \
  "$project_root/probes/OpenJdkNioMappingSmoke.java"; do
  [[ -e "$required" ]] || blocked "missing build dependency: $required"
done

cc="$(xcrun --find clang)"
libtool_bin="$(xcrun --find libtool)"
sdk_root="$(xcrun --sdk macosx --show-sdk-path)"
objects="$stage/objects"
mkdir -p "$objects"
common_flags=(
  -std=gnu11 -arch arm64 -isysroot "$sdk_root" -fPIC
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
)

while IFS= read -r source; do
  object="$objects/${source%.c}.o"
  source_path="$native_root/$source"
  if [[ "$source" == NativeThread.c ]]; then
    source_path="$patched_native_root/NativeThread.c"
  fi
  nio_abi_flags=()
  if [[ "$source" == FileDispatcherImpl.c || "$source" == FileChannelImpl.c ]]; then
    if [[ "$source" == FileDispatcherImpl.c ]]; then
      # FileDispatcherImpl preClose0 opens a host /dev/null with Darwin
      # constants. UnixNativeDispatcher_open0, built in its own gate, keeps
      # the Android-flag open wrapper.
      nio_abi_flags=(-DAIM_OPENJDK_NIO_HOST_OPEN)
    fi
    "$cc" "${common_flags[@]}" \
      "${nio_abi_flags[@]-}" \
      -I"$project_root/tools/bionic-errno-tls/include" \
      -I"$project_root/tools/bionic-fs-facade/include" \
      -I"$project_root/tools/bionic-ioctl-facade/include" \
      -I"$project_root/tools/bionic-socket-broker-adapter/include" \
      -include "$project_root/compat/darwin_openjdk_nio_fs_redirect.h" \
      -c "$source_path" -o "$object"
  else
    "$cc" "${common_flags[@]}" -c "$source_path" -o "$object"
  fi
  [[ "$(file "$object")" == *"Mach-O 64-bit object arm64"* ]] ||
    fail "non-arm64 target object: $source"
done < "$stage/target-sources.txt"

# The managed acceptance probe passes real host descriptors and intentionally
# exercises the unmodified OpenJDK implementation. Keep that probe isolated
# from the production virtual-fd redirect compiled above.
"$cc" "${common_flags[@]}" -c "$native_root/FileChannelImpl.c" \
  -o "$objects/FileChannelImpl-host-probe.o"
"$cc" "${common_flags[@]}" -c "$native_root/FileDispatcherImpl.c" \
  -o "$objects/FileDispatcherImpl-host-probe.o"

target_archive="$stage/libopenjdk-nio-mapping-darwin.a"
"$libtool_bin" -static -o "$target_archive" \
  "$objects/FileChannelImpl.o" "$objects/FileDispatcherImpl.o" \
  "$objects/NativeThread.o"
[[ "$(lipo -archs "$target_archive")" == arm64 ]] ||
  fail "target archive is not arm64"
[[ "$({ ar -t "$target_archive" || true; } | grep -v '^__\.SYMDEF' | \
      wc -l | tr -d ' ')" == "$TARGET_SOURCE_COUNT" ]] ||
  fail "target archive member count drift"
host_probe_archive="$stage/libopenjdk-nio-mapping-host-probe.a"
"$libtool_bin" -static -o "$host_probe_archive" \
  "$objects/FileChannelImpl-host-probe.o" \
  "$objects/FileDispatcherImpl-host-probe.o" \
  "$objects/NativeThread.o"
target_symbols="$stage/target-symbols.txt"
nm -g "$target_archive" > "$target_symbols"
for registrar in register_sun_nio_ch_FileChannelImpl \
  register_sun_nio_ch_FileDispatcherImpl register_sun_nio_ch_NativeThread; do
  grep -F "$registrar" "$target_symbols" >/dev/null ||
    fail "missing complete registrar: $registrar"
done
grep -F 'aim_restore_sun_nio_ch_NativeThread_signal' \
  "$target_symbols" >/dev/null ||
  fail "NativeThread archive lacks host signal restoration seam"
grep -F 'register_sun_nio_ch_IOUtil' "$target_symbols" >/dev/null &&
  fail "target archive unexpectedly owns IOUtil"
for provider in _fdval _convertReturnVal _convertLongReturnVal \
  _JNU_ThrowIOExceptionWithLastError _jniRegisterNativeMethods; do
  nm -u "$target_archive" | grep -F "$provider" >/dev/null ||
    fail "expected direct provider edge missing: $provider"
done

for source in IOUtil.c jni_util.c jni_util_md.c; do
  object="$objects/${source%.c}.o"
  "$cc" "${common_flags[@]}" -c "$native_root/$source" -o "$object"
done
support_archive="$stage/libopenjdk-nio-support-darwin.a"
"$libtool_bin" -static -o "$support_archive" \
  "$objects/IOUtil.o" "$objects/jni_util.o" "$objects/jni_util_md.o"
[[ "$(lipo -archs "$support_archive")" == arm64 ]] ||
  fail "support archive is not arm64"
[[ "$({ ar -t "$support_archive" || true; } | grep -v '^__\.SYMDEF' | \
      wc -l | tr -d ' ')" == 3 ]] || fail "support archive member count drift"
support_symbols="$stage/support-symbols.txt"
nm -g "$support_archive" > "$support_symbols"
for symbol in register_sun_nio_ch_IOUtil _fdval _convertReturnVal \
  _convertLongReturnVal _JNU_ThrowIOExceptionWithLastError; do
  grep -F "$symbol" "$support_symbols" >/dev/null ||
    fail "support provider missing: $symbol"
done
for duplicate in register_sun_nio_ch_FileChannelImpl \
  register_sun_nio_ch_FileDispatcherImpl register_sun_nio_ch_NativeThread; do
  grep -F "$duplicate" "$support_symbols" >/dev/null &&
    fail "support archive duplicates target owner: $duplicate"
done

python3 - "$native_root/IOUtil.c" "$stage/IOUtil-host-probe.c" <<'PY'
import sys
from pathlib import Path

source = Path(sys.argv[1]).read_text()
if source.count('"descriptor"') != 1:
    raise SystemExit('IOUtil FileDescriptor field contract drift')
Path(sys.argv[2]).write_text(source.replace('"descriptor"', '"fd"'))
PY
"$cc" "${common_flags[@]}" -c "$stage/IOUtil-host-probe.c" \
  -o "$objects/IOUtil-host-probe.o"
"$cc" "${common_flags[@]}" -c \
  "$project_root/probes/android16_openjdk_nio_mapping_jni.c" \
  -o "$objects/android16_openjdk_nio_mapping_jni.o"

managed_library="$stage/libandroid16-openjdk-nio-mapping-smoke.dylib"
"$cc" -arch arm64 -isysroot "$sdk_root" -dynamiclib \
  "$objects/android16_openjdk_nio_mapping_jni.o" \
  -Wl,-force_load,"$target_archive" \
  "$objects/IOUtil-host-probe.o" "$objects/jni_util.o" \
  "$objects/jni_util_md.o" \
  -Wl,-force_load,"$nativehelper_archive" "$liblog_archive" \
  -Wl,-exported_symbol,_JNI_OnLoad -Wl,-dead_strip \
  -Wl,-undefined,dynamic_lookup -framework CoreFoundation \
  -o "$managed_library"
otool -L "$managed_library" | grep -F '/opt/homebrew/' >/dev/null &&
  fail "forbidden Homebrew runtime dependency"

classes="$stage/classes"
mkdir -p "$classes"
javac --release 17 -encoding UTF-8 -d "$classes" \
  "$project_root/probes/OpenJdkNioMappingSmoke.java"
managed_output="$(java --add-opens java.base/sun.nio.ch=ALL-UNNAMED \
  --add-opens java.base/java.io=ALL-UNNAMED \
  -cp "$classes" dev.aim.probe.OpenJdkNioMappingSmoke \
  "$managed_library")"
expected='managed-openjdk-nio: methods=5+14+2 size=pass map-ro=pass unmap=pass virtual-readv=pass thread=pass signal-restore=pass'
[[ "$managed_output" == "$expected" ]] ||
  fail "managed acceptance failed: $managed_output"

mkdir -p "$build_dir"
cp "$target_archive" "$build_dir/libopenjdk-nio-mapping-darwin.a"
cp "$support_archive" "$build_dir/libopenjdk-nio-support-darwin.a"
echo "openjdk-nio-mapping: methods=5+14+2 sources=3 providers=IOUtil+jni_util size=pass map-ro=pass unmap=pass virtual-readv=pass thread=pass signal-restore=pass archives=Mach-O-arm64"
