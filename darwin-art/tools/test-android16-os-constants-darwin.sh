#!/bin/bash
set -euo pipefail
export LC_ALL=C

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/.." && pwd)"
lock_file="$project_root/upstream/android16-os-constants.lock"
# shellcheck disable=SC1090
source "$lock_file"

sha256() { shasum -a 256 "$1" | awk '{print $1}'; }
fail() { echo "os-constants-test: $*" >&2; exit 3; }

if [[ $# == 0 ]]; then
  bash "$script_dir/build-android16-os-constants-darwin.sh" --archive-only
elif [[ $# != 1 || "$1" != --prepared ]]; then
  echo 'usage: test-android16-os-constants-darwin.sh [--prepared]' >&2
  exit 2
fi

build_dir="$project_root/_build/os-constants"
archive="$build_dir/libandroid-system-os-constants-darwin.a"
values="$build_dir/android16-os-constants-values.tsv"
generated_dir="$build_dir/generated"
nativehelper_source="$project_root/_aosp/libnativehelper-full"
nativehelper_archive="$project_root/_build/nativehelper-foundation/libnativehelper_jvm.a"
liblog_include="$project_root/_aosp/system/logging/liblog/include"
liblog_archive="$project_root/_build/graphics-foundations/liblog-darwin.a"
for required in \
  "$archive" "$values" \
  "$generated_dir/android16_os_constants_values.inc" \
  "$generated_dir/android16_os_constants_errno.inc" \
  "$generated_dir/android16_os_constants_sysconf.inc" \
  "$generated_dir/names.txt" "$generated_dir/expressions.tsv" \
  "$generated_dir/derived-values.tsv" \
  "$project_root/probes/android16_os_constants_jni.cc" \
  "$project_root/compat/darwin_os_constants.h" \
  "$nativehelper_source/include_jni/jni.h" \
  "$nativehelper_source/include/nativehelper/JNIHelp.h" \
  "$nativehelper_source/include/nativehelper/ScopedUtfChars.h" \
  "$nativehelper_archive" "$liblog_include/android/log.h" "$liblog_archive"; do
  [[ -e "$required" ]] || { echo "os-constants-test: missing $required" >&2; exit 2; }
done

[[ "$(sha256 "$values")" == "$ANDROID_VALUES_SHA256" ]] ||
  fail "published Android value manifest drift"
[[ "$(wc -l < "$values" | tr -d " ")" == "$CONSTANT_COUNT" ]] ||
  fail "published Android value count drift"
[[ "$(sha256 "$generated_dir/names.txt")" == "$CONSTANT_NAMES_SHA256" ]] ||
  fail "published constant name manifest drift"
[[ "$(sha256 "$generated_dir/expressions.tsv")" == "$CONSTANT_EXPRESSIONS_SHA256" ]] ||
  fail "published constant expression manifest drift"
cmp -s "$generated_dir/derived-values.tsv" "$values" ||
  fail "published derived Android values differ from pinned manifest"
[[ "$(sha256 "$generated_dir/derived-values.tsv")" == "$ANDROID_VALUES_SHA256" ]] ||
  fail "published derived value manifest drift"
[[ "$(grep -c '^    {"' "$generated_dir/android16_os_constants_values.inc")" == "$CONSTANT_COUNT" ]] ||
  fail "published value include count drift"
[[ "$(grep -c '^#ifdef ' "$generated_dir/android16_os_constants_errno.inc")" == "$ERRNO_CONSTANT_COUNT" ]] ||
  fail "published errno include count drift"
[[ "$(grep -c '^#ifdef ' "$generated_dir/android16_os_constants_sysconf.inc")" == "$SYSCONF_CONSTANT_COUNT" ]] ||
  fail "published sysconf include count drift"

stage="$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-os-constants-test.XXXXXX")"
trap 'rm -rf "$stage"' EXIT
cc="$(xcrun --find clang++)"
sdk_root="$(xcrun --sdk macosx --show-sdk-path)"
common_flags=(
  -std=c++20 -arch arm64 -isysroot "$sdk_root" -fPIC
  -Wall -Wextra -Werror
  -I"$project_root/compat" -I"$generated_dir"
  -I"$nativehelper_source/include_jni"
  -I"$nativehelper_source/include"
  -I"$nativehelper_source/include_platform"
  -I"$nativehelper_source/include_platform_header_only"
  -I"$nativehelper_source/header_only_include"
  -I"$liblog_include"
)
"$cc" "${common_flags[@]}" -c \
  "$project_root/probes/android16_os_constants_jni.cc" \
  -o "$stage/android16_os_constants_jni.o"
managed_library="$stage/libandroid16-os-constants-smoke.dylib"
"$cc" -arch arm64 -isysroot "$sdk_root" -dynamiclib \
  "$stage/android16_os_constants_jni.o" -Wl,-force_load,"$archive" \
  -Wl,-force_load,"$nativehelper_archive" "$liblog_archive" \
  -Wl,-exported_symbol,_JNI_OnLoad -Wl,-dead_strip \
  -Wl,-undefined,dynamic_lookup -framework CoreFoundation \
  -o "$managed_library"
otool -L "$managed_library" | grep -F '/opt/homebrew/' >/dev/null &&
  fail "forbidden Homebrew runtime dependency"
java_root="$stage/java"
classes="$stage/classes"
mkdir -p "$java_root/android/system" "$java_root/dev/darwinart/probe" "$classes"
python3 - "$values" > "$java_root/android/system/OsConstants.java" <<'PY'
import sys
from pathlib import Path

entries = [line.rstrip('\n').split('\t') for line in Path(sys.argv[1]).open()]
print('package android.system;')
print('public final class OsConstants {')
print('  private static int placeholder() { return 0; }')
print('  private static native void initConstants();')
for name, _ in entries:
    print(f'  public static final int {name} = placeholder();')
print('  public static boolean S_ISDIR(int mode) { return (mode & S_IFMT) == S_IFDIR; }')
print('  public static boolean S_ISREG(int mode) { return (mode & S_IFMT) == S_IFREG; }')
print('  static { System.load(System.getProperty("os.constants.library")); initConstants(); }')
print('}')
PY
cat > "$java_root/dev/darwinart/probe/OsConstantsProbe.java" <<'JAVA'
package dev.darwinart.probe;

import android.system.OsConstants;
import java.lang.reflect.Field;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.HashMap;
import java.util.List;
import java.util.Map;

public final class OsConstantsProbe {
  private static native int createWithAndroidFlags(String path, int flags);
  private static native int statMode(String path);
  private static native int missingPathErrno(String path);
  private static native int androidNotSupportedErrno();
  private static native long processorCount(int name);

  private static void require(boolean condition, String message) {
    if (!condition) throw new AssertionError(message);
  }

  public static void main(String[] args) throws Exception {
    System.setProperty("os.constants.library", args[0]);
    List<String> lines = Files.readAllLines(Path.of(args[1]));
    Map<String, Integer> expected = new HashMap<>();
    for (String line : lines) {
      String[] parts = line.split("\\t");
      expected.put(parts[0], Integer.parseInt(parts[1]));
    }
    int checked = 0;
    for (Field field : OsConstants.class.getFields()) {
      if (field.getType() != int.class) continue;
      Integer value = expected.get(field.getName());
      require(value != null, "unexpected field " + field.getName());
      require(field.getInt(null) == value, "value mismatch " + field.getName());
      checked++;
    }
    require(checked == 568 && expected.size() == 568, "incomplete constant table");
    require(OsConstants.S_IFMT == 61440 && OsConstants.S_IFDIR == 16384 &&
            OsConstants.S_IFREG == 32768, "Linux stat ABI values changed");
    require(OsConstants.ENOENT == 2 && OsConstants.ENOTSUP == 95 &&
            OsConstants.EAGAIN == 11, "Linux errno ABI values changed");
    require(OsConstants.O_CREAT == 64 && OsConstants.O_TRUNC == 512 &&
            OsConstants.O_CLOEXEC == 524288 && OsConstants.O_DIRECT == 65536,
            "Linux open ABI values changed");
    require(OsConstants._SC_NPROCESSORS_CONF == 96,
            "Linux sysconf ABI value changed");

    Path path = Files.createTempFile("darwin-art-os-constants", ".tmp");
    Files.delete(path);
    int flags = OsConstants.O_CREAT | OsConstants.O_TRUNC |
        OsConstants.O_WRONLY | OsConstants.O_CLOEXEC;
    require(createWithAndroidFlags(path.toString(), flags) == 0,
            "Android open flags were not translated");
    int mode = statMode(path.toString());
    require(mode >= 0 && OsConstants.S_ISREG(mode) && !OsConstants.S_ISDIR(mode),
            "regular file misclassified as directory");
    Path missing = path.resolveSibling(path.getFileName() + ".missing");
    require(missingPathErrno(missing.toString()) == OsConstants.ENOENT,
            "Darwin ENOENT was not translated");
    require(androidNotSupportedErrno() == OsConstants.ENOTSUP,
            "Darwin ENOTSUP was not translated to Android 95");
    require(processorCount(OsConstants._SC_NPROCESSORS_CONF) > 0,
            "Android sysconf name was not translated");
    Files.delete(path);
    System.out.println("managed-os-constants: fields=568 stat=regular open=pass errno=pass sysconf=pass");
  }
}
JAVA

javac --release 17 -encoding UTF-8 -d "$classes" \
  "$java_root/android/system/OsConstants.java" \
  "$java_root/dev/darwinart/probe/OsConstantsProbe.java"
managed_output="$(java -cp "$classes" \
  dev.darwinart.probe.OsConstantsProbe "$managed_library" "$values")"
expected_output='managed-os-constants: fields=568 stat=regular open=pass errno=pass sysconf=pass'
[[ "$managed_output" == "$expected_output" ]] ||
  fail "managed acceptance failed: $managed_output"

echo "os-constants-test: archive=provided fields=$CONSTANT_COUNT Android-ABI=pass translations=open+errno+sysconf managed=pass"
