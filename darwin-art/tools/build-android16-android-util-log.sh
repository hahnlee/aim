#!/bin/bash
set -euo pipefail
export LC_ALL=C

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/.." && pwd)"
lock_file="$project_root/upstream/android16-android-util-log.lock"
source_root="$project_root/_aosp/frameworks-base-android-util-log"
build_dir="$project_root/_build/android-util-log"

# shellcheck disable=SC1090
source "$lock_file"

sha256() { shasum -a 256 "$1" | awk '{print $1}'; }
fail() { echo "android-util-log: $*" >&2; exit 3; }

verify_hash() {
  local source_file="$1" expected="$2"
  [[ -f "$source_file" ]] || {
    echo "android-util-log: missing dependency: $source_file" >&2
    exit 2
  }
  [[ "$(sha256 "$source_file")" == "$expected" ]] ||
    fail "checksum mismatch: $source_file"
}

materialize() {
  local relative="$1" expected="$2"
  local destination="$source_root/$relative"
  if [[ -f "$destination" ]]; then
    verify_hash "$destination" "$expected"
    return
  fi
  mkdir -p "$(dirname "$destination")"
  local staged
  staged="$(mktemp "${destination}.download.XXXXXX")"
  curl -fsSL \
    "https://android.googlesource.com/$FRAMEWORKS_BASE_PROJECT/+/$FRAMEWORKS_BASE_REVISION/$relative?format=TEXT" \
    | base64 -D > "$staged"
  [[ "$(sha256 "$staged")" == "$expected" ]] || {
    fail "download checksum mismatch: $relative"
  }
  mv "$staged" "$destination"
}

materialize core/jni/Android.bp "$ANDROID_BP_SHA256"
materialize core/jni/android_util_Log.cpp "$LOG_CPP_SHA256"
materialize core/jni/android_util_Log.h "$LOG_H_SHA256"
materialize core/jni/core_jni_helpers.h "$CORE_JNI_HELPERS_SHA256"
materialize core/jni/jni_wrappers.h "$JNI_WRAPPERS_SHA256"
materialize core/jni/include/android_runtime/AndroidRuntime.h \
  "$ANDROID_RUNTIME_H_SHA256"
materialize core/jni/platform/host/HostRuntime.cpp "$HOST_RUNTIME_CPP_SHA256"
materialize core/jni/AndroidRuntime.cpp "$ANDROID_RUNTIME_CPP_SHA256"
materialize core/java/android/util/Log.java "$LOG_JAVA_SHA256"
materialize core/jni/android_util_EventLog.cpp "$EVENTLOG_CPP_SHA256"
materialize core/jni/eventlog_helper.h "$EVENTLOG_HELPER_H_SHA256"

android_bp="$source_root/core/jni/Android.bp"
log_source="$source_root/core/jni/android_util_Log.cpp"
eventlog_source="$source_root/core/jni/android_util_EventLog.cpp"
host_runtime="$source_root/core/jni/platform/host/HostRuntime.cpp"
android_runtime="$source_root/core/jni/AndroidRuntime.cpp"
managed_log="$source_root/core/java/android/util/Log.java"
stage="$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-android-util-log.XXXXXX")"
trap 'rm -rf "$stage"' EXIT

python3 - "$android_bp" "$OWNER_MODULE" "$OWNER_SOURCE" <<'PY'
import sys
from pathlib import Path

text = Path(sys.argv[1]).read_text()
module = sys.argv[2]
source = sys.argv[3]
start = text.index(f'name: "{module}"')
body_start = text.index('srcs: [', start)
body_end = text.index('],', body_start)
sources = text[body_start:body_end]
assert sources.count(f'"{source}"') == 1
assert text.index('host_supported: true', start) < body_start
PY

method_manifest="$stage/native-methods.tsv"
python3 - "$log_source" > "$method_manifest" <<'PY'
import re
import sys
from pathlib import Path

text = Path(sys.argv[1]).read_text()
start = text.index('static const JNINativeMethod gMethods[]')
end = text.index('\n};', start)
for name, signature in re.findall(r'\{\s*"([^"]+)"\s*,\s*"([^"]+)"', text[start:end]):
    print(f'{name}\t{signature}')
PY
method_count="$(wc -l < "$method_manifest" | tr -d ' ')"
method_sha="$(sha256 "$method_manifest")"
[[ "$method_count" == "$NATIVE_METHOD_COUNT" &&
   "$method_sha" == "$NATIVE_METHOD_MANIFEST_SHA256" ]] ||
  fail "native table changed count=$method_count sha=$method_sha"

eventlog_methods="$(python3 - "$eventlog_source" <<'PY'
import re
import sys
from pathlib import Path

text = Path(sys.argv[1]).read_text()
start = text.index('static const JNINativeMethod gRegisterMethods[]')
end = text.index('\n};', start)
print(len(re.findall(r'\{\s*"([^"]+)"\s*,', text[start:end])))
PY
)"
[[ "$eventlog_methods" == "$EVENTLOG_NATIVE_METHOD_COUNT" ]] ||
  fail "EventLog native table changed count=$eventlog_methods"

for declaration in \
  'native boolean isLoggable' \
  'native int println_native' \
  'native int logger_entry_max_payload_native'; do
  grep -F "$declaration" "$managed_log" >/dev/null ||
    fail "managed native declaration missing: $declaration"
done

python3 - "$host_runtime" "$android_runtime" <<'PY'
import sys
from pathlib import Path

host = Path(sys.argv[1]).read_text()
map_start = host.index('static const std::unordered_map<std::string, RegJNIRec> gRegJNIMap')
map_end = host.index('\n};', map_start)
registry = host[map_start:map_end]
assert registry.count('{"android.util.Log", REG_JNI(register_android_util_Log)}') == 1
assert 'for (const string& className : classesToRegister)' in host
assert 'jniRegMap.at(className).mProc(env)' in host
assert 'getJavaProperty(env, "core_native_classes")' in host
device = Path(sys.argv[2]).read_text()
table = device[device.index('static const RegJNIRec gRegJNI[]'):]
assert table.index('register_android_util_CharsetUtils') < \
       table.index('register_android_util_EventLog') < \
       table.index('register_android_util_Log') < \
       table.index('register_android_util_MemoryIntArray')
PY

liblog_root="$project_root/_aosp/system/logging/liblog"
libutils_root="$project_root/_aosp/system/core/libutils"
libsystem_root="$project_root/_aosp/system/core/libsystem"
libbase_root="$project_root/_aosp/system/libbase"
nativehelper="$project_root/_build/nativehelper-foundation/source/libnativehelper"
liblog_archive="$project_root/_build/graphics-foundations/liblog-darwin.a"
verify_hash "$liblog_root/Android.bp" "$LIBLOG_ANDROID_BP_SHA256"
verify_hash "$liblog_root/include/log/log.h" "$LIBLOG_PUBLIC_HEADER_SHA256"
verify_hash "$libutils_root/include/utils/Log.h" "$LIBUTILS_LOG_H_SHA256"
verify_hash "$libutils_root/include/utils/String8.h" "$LIBUTILS_STRING8_H_SHA256"
verify_hash "$libutils_root/include/utils/misc.h" "$LIBUTILS_MISC_H_SHA256"
verify_hash "$libsystem_root/include/system/graphics.h" \
  "$LIBSYSTEM_GRAPHICS_H_SHA256"
verify_hash "$libbase_root/include/android-base/macros.h" \
  "$LIBBASE_MACROS_H_SHA256"
verify_hash "$nativehelper/include/nativehelper/JNIHelp.h" "$JNI_HELP_H_SHA256"
verify_hash "$nativehelper/include_platform/nativehelper/JNIPlatformHelp.h" \
  "$JNI_PLATFORM_HELP_H_SHA256"
verify_hash "$nativehelper/include_jni/jni.h" "$JNI_H_SHA256"
[[ -f "$project_root/_aosp/libnativehelper-full/.source-revision" &&
   "$(<"$project_root/_aosp/libnativehelper-full/.source-revision")" == \
     "$LIBNATIVEHELPER_REVISION" ]] ||
  fail "libnativehelper revision mismatch"
for revision_file in \
  "$liblog_root/.source-revision:$SYSTEM_LOGGING_REVISION" \
  "$libutils_root/.source-revision:$SYSTEM_CORE_REVISION" \
  "$libsystem_root/.source-revision:$SYSTEM_CORE_REVISION" \
  "$libbase_root/.source-revision:$LIBBASE_REVISION"; do
  file_name="${revision_file%%:*}"
  expected_revision="${revision_file#*:}"
  [[ -f "$file_name" && "$(<"$file_name")" == "$expected_revision" ]] ||
    fail "dependency revision mismatch: $file_name"
done
for required in \
  "$project_root/probes/android16_android_util_log_jni.cc" \
  "$nativehelper/include_jni/jni.h" \
  "$nativehelper/include/nativehelper/JNIHelp.h" \
  "$nativehelper/include_platform/nativehelper/JNIPlatformHelp.h" \
  "$liblog_archive"; do
  [[ -e "$required" ]] || {
    echo "android-util-log: missing dependency: $required" >&2
    exit 2
  }
done

cxx="$(xcrun --find clang++)"
libtool_bin="$(xcrun --find libtool)"
sdk_root="$(xcrun --sdk macosx --show-sdk-path)"
common_flags=(
  -std=c++20 -arch arm64 -isysroot "$sdk_root" -fPIC
  -Wall -Wextra -Werror -Wno-unused-parameter -Wno-writable-strings
  -I"$source_root/core/jni"
  -I"$source_root/core/jni/include"
  -I"$nativehelper/include_jni"
  -I"$nativehelper/include"
  -I"$nativehelper/include_platform"
  -I"$nativehelper/header_only_include"
  -I"$liblog_root/include"
  -I"$libutils_root/include"
  -I"$libsystem_root/include"
  -I"$libbase_root/include"
  -I"$project_root/_aosp/system/core/libcutils/include"
)

object="$stage/android_util_Log.o"
"$cxx" "${common_flags[@]}" -c "$log_source" -o "$object"
[[ "$(file "$object")" == *"Mach-O 64-bit object arm64"* ]] ||
  fail "registrar object is not Darwin arm64"
eventlog_object="$stage/android_util_EventLog.o"
"$cxx" "${common_flags[@]}" -c "$eventlog_source" -o "$eventlog_object"
archive="$stage/libandroid-util-log-registrar-darwin.a"
"$libtool_bin" -static -o "$archive" "$object" "$eventlog_object"
member_count="$({ ar -t "$archive" || true; } | grep -v '^__\.SYMDEF' | wc -l | tr -d ' ')"
[[ "$member_count" == 2 && "$(lipo -archs "$archive")" == arm64 ]] ||
  fail "registrar archive identity mismatch"
definitions="$stage/definitions.txt"
nm -gU "$archive" | c++filt | sort -u > "$definitions"
grep -F ' T android::register_android_util_Log(_JNIEnv*)' \
  "$definitions" >/dev/null || fail "registrar definition missing"
grep -F ' T android::android_util_Log_isVerboseLogEnabled(char const*)' \
  "$definitions" >/dev/null || fail "verbose-log definition missing"
grep -F ' T android::register_android_util_EventLog(_JNIEnv*)' \
  "$definitions" >/dev/null || fail "EventLog registrar definition missing"

log_imports="$stage/android-log-imports.txt"
nm -u "$object" | grep '^___android_log_' | sort -u > "$log_imports"
log_import_count="$(wc -l < "$log_imports" | tr -d ' ')"
log_import_sha="$(sha256 "$log_imports")"
[[ "$log_import_count" == "$ANDROID_LOG_IMPORT_COUNT" &&
   "$log_import_sha" == "$ANDROID_LOG_IMPORTS_SHA256" ]] ||
  fail "liblog import identity changed count=$log_import_count sha=$log_import_sha"
while IFS= read -r symbol; do
  nm -gU "$liblog_archive" | awk '$2 ~ /^[TDBSCRGWV]$/ {print $3}' | \
    grep -Fx "$symbol" >/dev/null || fail "liblog provider missing: $symbol"
done < "$log_imports"

jni_object="$stage/android_util_log_jni.o"
"$cxx" "${common_flags[@]}" \
  -c "$project_root/probes/android16_android_util_log_jni.cc" \
  -o "$jni_object"
managed_library="$stage/libandroid-util-log-managed.dylib"
"$cxx" -arch arm64 -isysroot "$sdk_root" -dynamiclib \
  "$jni_object" "$object" "$liblog_archive" -o "$managed_library"

java_sources="$stage/java-sources"
java_classes="$stage/java-classes"
mkdir -p "$java_sources/android/util" "$java_sources/dev/darwinart/probe" \
  "$java_classes"
cat > "$java_sources/android/util/Log.java" <<'JAVA'
package android.util;

public final class Log {
    public static final int VERBOSE = 2;
    public static final int DEBUG = 3;
    public static final int INFO = 4;
    public static final int WARN = 5;
    public static final int ERROR = 6;
    public static final int ASSERT = 7;

    public static native boolean isLoggable(String tag, int level);
    public static native int println_native(int buffer, int priority, String tag, String message);
    private static native int logger_entry_max_payload_native();

    public static int loggerEntryMaxPayload() {
        return logger_entry_max_payload_native();
    }
}
JAVA
cat > "$java_sources/dev/darwinart/probe/AndroidUtilLogSmoke.java" <<'JAVA'
package dev.darwinart.probe;

import android.util.Log;

public final class AndroidUtilLogSmoke {
    public static void main(String[] args) {
        if (args.length != 1) throw new IllegalArgumentException("dylib required");
        System.load(args[0]);
        int payload = Log.loggerEntryMaxPayload();
        if (payload != 4068) throw new AssertionError("payload=" + payload);
        boolean loggable = Log.isLoggable("DarwinArtLogGate", Log.INFO);
        int written = Log.println_native(0, Log.INFO, "DarwinArtLogGate",
                "full native table smoke");
        if (written < 0) throw new AssertionError("println=" + written);
        try {
            Log.println_native(0, Log.INFO, "DarwinArtLogGate", null);
            throw new AssertionError("null message did not throw");
        } catch (NullPointerException expected) {
            // Exact upstream contract.
        }
        System.out.println("android-util-log: methods=3 payload=" + payload
                + " loggable-info=" + loggable + " println=pass null=NPE");
    }
}
JAVA
javac --release 17 -encoding UTF-8 -d "$java_classes" \
  "$java_sources/android/util/Log.java" \
  "$java_sources/dev/darwinart/probe/AndroidUtilLogSmoke.java"
managed_output="$(java -cp "$java_classes" \
  dev.darwinart.probe.AndroidUtilLogSmoke "$managed_library")"
[[ "$managed_output" == android-util-log:*' println=pass null=NPE' ]] ||
  fail "managed full-table smoke failed: $managed_output"

linked_libraries="$(otool -L "$managed_library")"
if grep -E '(/opt/homebrew|liblog\.(dylib|so))' <<< "$linked_libraries" >/dev/null; then
  fail "forbidden external logging provider linked"
fi

mkdir -p "$build_dir"
cp "$archive" "$build_dir/libandroid-util-log-registrar-darwin.a"
echo "android-util-log: owner=$OWNER_MODULE members=2 eventlog-methods=$eventlog_methods methods=$method_count liblog-imports=$log_import_count managed=pass payload=$LOGGER_ENTRY_MAX_PAYLOAD"
