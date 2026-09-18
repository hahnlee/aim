#!/bin/bash
set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/.." && pwd)"
lock_file="$project_root/upstream/android16-libcore-darwin-linux.lock"
# shellcheck disable=SC1090
source "$lock_file"

sha256() { shasum -a 256 "$1" | awk '{print $1}'; }
fail() { echo "libcore-darwin-linux-test: $*" >&2; exit 3; }

if [[ $# == 0 ]]; then
  bash "$script_dir/build-android16-libcore-darwin-linux.sh" --archive-only
elif [[ $# != 1 || "$1" != --prepared ]]; then
  echo 'usage: test-android16-libcore-darwin-linux.sh [--prepared]' >&2
  exit 2
fi

source_file="$project_root/_aosp/libcore/$LIBCORE_LINUX_CPP"
if [[ ! -f "$source_file" ]] ||
   [[ "$(sha256 "$source_file")" != "$LIBCORE_LINUX_CPP_SHA256" ]]; then
  mkdir -p "$(dirname "$source_file")"
  staged_source="$(mktemp "${source_file}.download.XXXXXX")"
  curl -fsSL \
    "https://android.googlesource.com/$LIBCORE_PROJECT/+/$LIBCORE_REVISION/$LIBCORE_LINUX_CPP?format=TEXT" \
    | base64 -D > "$staged_source"
  [[ "$(sha256 "$staged_source")" == "$LIBCORE_LINUX_CPP_SHA256" ]] ||
    fail "upstream source checksum mismatch"
  mv "$staged_source" "$source_file"
fi

stage="$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-libcore-linux.XXXXXX")"
trap 'rm -rf "$stage"' EXIT
manifest="$stage/methods.tsv"
perl -0777 -ne '
  if (/static JNINativeMethod gMethods\[\] = \{(.*?)\n\};/s) {
    $body = $1;
    while ($body =~ /(CRITICAL_NATIVE_METHOD|NATIVE_METHOD(?:_OVERLOAD)?)\(Linux,\s*(\w+),\s*"([^"]+)"(?:,\s*(\w+))?\)/gs) {
      print "$1\t$2\t$3\n";
    }
  }
' "$source_file" > "$manifest"
method_count="$(wc -l < "$manifest" | tr -d ' ')"
[[ "$method_count" == "$LIBCORE_LINUX_METHOD_COUNT" ]] ||
  fail "method count=$method_count expected=$LIBCORE_LINUX_METHOD_COUNT"
[[ "$(sha256 "$manifest")" == "$LIBCORE_LINUX_METHOD_MANIFEST_SHA256" ]] ||
  fail "method manifest drift"

build_dir="$project_root/_build/libcore-darwin-linux"
archive="$build_dir/libcore-darwin-linux.a"
method_include_artifact="$build_dir/darwin_linux_method_table.inc"
methods_manifest_artifact="$build_dir/libcore-darwin-linux-methods.tsv"
abi_manifest_artifact="$build_dir/libcore-darwin-linux-unsupported-abi.tsv"
counts_artifact="$build_dir/libcore-darwin-linux-counts.txt"
nativehelper="$project_root/_build/nativehelper-foundation/source/libnativehelper"
nativehelper_archive="$project_root/_build/nativehelper-foundation/libnativehelper_jvm.a"
liblog_archive="$project_root/_build/graphics-foundations/liblog-darwin.a"
async_close_archive="$project_root/_build/asynchronous-close-monitor/libandroidio-darwin.a"
os_constants_archive="$project_root/_build/os-constants/libandroid-system-os-constants-darwin.a"
for required in \
  "$archive" "$method_include_artifact" "$methods_manifest_artifact" \
  "$abi_manifest_artifact" "$counts_artifact" \
  "$project_root/compat/libcore_darwin_linux.cc" \
  "$project_root/compat/libcore_darwin_linux_system_natives.cc" \
  "$project_root/compat/libcore_darwin_linux_syscalls.cc" \
  "$project_root/compat/libcore_darwin_linux.h" \
  "$project_root/compat/darwin_os_constants.h" \
  "$project_root/probes/android16_libcore_darwin_linux_smoke.cc" \
  "$nativehelper/include/nativehelper/JNIHelp.h" \
  "$nativehelper/include_platform/nativehelper/JNIPlatformHelp.h" \
  "$nativehelper/include_jni/jni.h" "$nativehelper_archive" \
  "$async_close_archive" "$os_constants_archive" "$liblog_archive"; do
  [[ -e "$required" ]] || { echo "libcore-darwin-linux-test: missing $required" >&2; exit 2; }
done

cmp -s "$manifest" "$methods_manifest_artifact" ||
  fail "published JNI method manifest differs from source audit"
[[ "$(sha256 "$methods_manifest_artifact")" == "$LIBCORE_LINUX_METHOD_MANIFEST_SHA256" ]] ||
  fail "published method manifest drift"
artifact_method_count="$(grep -Fc 'const_cast<char*>' "$method_include_artifact")"
[[ "$artifact_method_count" == "$method_count" ]] ||
  fail "published method table count=$artifact_method_count expected=$method_count"
grep -F '...' "$method_include_artifact" >/dev/null &&
  fail "published method table contains a variadic parameter"
supported_regular="$(awk -F= '$1 == "regular" { print $2 }' "$counts_artifact")"
supported_critical="$(awk -F= '$1 == "critical" { print $2 }' "$counts_artifact")"
unsupported_count="$(awk -F= '$1 == "unsupported" { print $2 }' "$counts_artifact")"
[[ "$supported_regular" == "$LIBCORE_DARWIN_SUPPORTED_REGULAR" ]] ||
  fail "supported regular count=$supported_regular"
[[ "$supported_critical" == "$LIBCORE_DARWIN_SUPPORTED_CRITICAL" ]] ||
  fail "supported critical count=$supported_critical"
[[ "$unsupported_count" == "$((method_count - supported_regular - supported_critical))" ]] ||
  fail "unsupported method count=$unsupported_count"
[[ "$(wc -l < "$abi_manifest_artifact" | tr -d ' ')" == "$unsupported_count" ]] ||
  fail "unsupported ABI manifest count mismatch"
grep -F $'getsockoptByte\t(Ljava/io/FileDescriptor;II)I\tjint\tJNIEnv* env,jobject,jobject,jint,jint' \
  "$abi_manifest_artifact" >/dev/null || fail "representative fixed ABI missing"

sdk_root="$(xcrun --sdk macosx --show-sdk-path)"
cxx="$(xcrun --find clang++)"
common_flags=(
  -std=c++20 -arch arm64 -isysroot "$sdk_root" -fPIC -Wall -Wextra -Werror
  -I"$project_root/compat"
  -I"$project_root/tools/bionic-socket-broker-adapter/include"
  -I"$project_root/tools/bionic-process-state-facade/include"
  -I"$project_root/tools/bionic-fs-facade/include"
  -I"$project_root/tools/bionic-ioctl-facade/include"
  -I"$build_dir"
  -I"$nativehelper/include_jni"
  -I"$nativehelper/include"
  -I"$nativehelper/include_platform"
  -I"$nativehelper/header_only_include"
  -I"$project_root/_aosp/system/logging/liblog/include"
)
syscalls_object="$stage/libcore_darwin_linux_syscalls.o"
system_object="$stage/libcore_darwin_linux_system_natives.o"
"$cxx" "${common_flags[@]}" -c \
  "$project_root/compat/libcore_darwin_linux_syscalls.cc" -o "$syscalls_object"
"$cxx" "${common_flags[@]}" -c \
  "$project_root/compat/libcore_darwin_linux_system_natives.cc" -o "$system_object"
smoke="$stage/libcore-darwin-linux-smoke"
"$cxx" "${common_flags[@]}" \
  "$project_root/probes/android16_libcore_darwin_linux_smoke.cc" \
  "$archive" "$async_close_archive" "$os_constants_archive" \
  -Wl,-force_load,"$nativehelper_archive" \
  "$liblog_archive" -Wl,-undefined,dynamic_lookup -o "$smoke"
smoke_output="$($smoke "$source_file")"
[[ "$smoke_output" == libcore-darwin-linux:* ]] || fail "smoke output mismatch"

abi_object="$stage/libcore_darwin_linux_abi_smoke.o"
"$cxx" "${common_flags[@]}" -DDARWIN_LIBCORE_LINUX_MANAGED_ABI_SMOKE=1 \
  -c "$project_root/compat/libcore_darwin_linux.cc" -o "$abi_object"
abi_library="$stage/libcore-darwin-linux-abi-smoke.dylib"
"$cxx" -arch arm64 -isysroot "$sdk_root" -dynamiclib "$abi_object" \
  "$system_object" "$syscalls_object" \
  "$async_close_archive" "$os_constants_archive" \
  -Wl,-force_load,"$nativehelper_archive" \
  "$liblog_archive" -Wl,-undefined,dynamic_lookup -o "$abi_library"
nm -gU "$abi_library" | grep -F ' _JNI_OnLoad' >/dev/null ||
  fail "managed ABI smoke JNI_OnLoad is missing"

java_sources="$stage/java-sources"
java_classes="$stage/java-classes"
mkdir -p "$java_sources/android/system" "$java_sources/dev/darwinart/probe" \
  "$java_classes"
cat > "$java_sources/android/system/StructTimespec.java" <<'JAVA'
package android.system;

public final class StructTimespec {
    public final long tv_sec;
    public final long tv_nsec;

    public StructTimespec(long seconds, long nanoseconds) {
        tv_sec = seconds;
        tv_nsec = nanoseconds;
    }
}
JAVA
cat > "$java_sources/android/system/StructStat.java" <<'JAVA'
package android.system;

public final class StructStat {
    public final long st_size;

    public StructStat(long device, long inode, int mode, long links, int uid, int gid,
            long rdev, long size, StructTimespec atime, StructTimespec mtime,
            StructTimespec ctime, long blockSize, long blocks) {
        st_size = size;
    }
}
JAVA
cat > "$java_sources/android/system/StructUtsname.java" <<'JAVA'
package android.system;

public final class StructUtsname {
    public final String sysname;
    public final String machine;

    public StructUtsname(String sysname, String nodename, String release,
            String version, String machine) {
        this.sysname = sysname;
        this.machine = machine;
    }
}
JAVA
cat > "$java_sources/android/system/ErrnoException.java" <<'JAVA'
package android.system;

public final class ErrnoException extends Exception {
    public final int errno;

    public ErrnoException(String functionName, int errno) {
        super(functionName + " failed with errno " + errno);
        this.errno = errno;
    }
}
JAVA
cat > "$java_sources/dev/darwinart/probe/LibcoreDarwinAbiSmoke.java" <<'JAVA'
package dev.darwinart.probe;

import android.system.ErrnoException;
import android.system.StructStat;
import android.system.StructUtsname;
import java.io.File;
import java.io.FileDescriptor;
import java.nio.charset.StandardCharsets;

public final class LibcoreDarwinAbiSmoke {
    private native void unsupportedVoid(String value, int number) throws ErrnoException;
    private native int unsupportedInt(FileDescriptor fd, int first, int second)
            throws ErrnoException;
    private native long unsupportedLong(FileDescriptor fd) throws ErrnoException;
    private native String unsupportedObject(String value) throws ErrnoException;
    private native boolean unsupportedBoolean(String value, int number) throws ErrnoException;
    private native long availableProcessors() throws ErrnoException;
    private native String environment(String name) throws ErrnoException;
    private native void setEnvironment(String name, String value, boolean overwrite)
            throws ErrnoException;
    private native void unsetEnvironment(String name) throws ErrnoException;
    private native StructStat statPath(String path) throws ErrnoException;
    private native boolean accessPath(String path, int mode) throws ErrnoException;
    private native int writeFile(String path, byte[] bytes) throws ErrnoException;
    private native StructUtsname unameView() throws ErrnoException;
    private native String errorMessage(int errorNumber);
    private native String signalMessage(int signalNumber);
    private native FileDescriptor openFile(String path, int flags, int mode)
            throws ErrnoException;
    private native long seekFile(FileDescriptor fd, long offset, int whence)
            throws ErrnoException;
    private native int readFile(FileDescriptor fd, Object bytes, int offset, int count)
            throws ErrnoException;
    private native void closeFile(FileDescriptor fd) throws ErrnoException;
    private static native void exchangeOwnerTag(FileDescriptor fd, long expected, long replacement);
    private static native long getOwnerTag(FileDescriptor fd);
    private static native String getTagType(long tag);
    private static native long getTagValue(long tag);

    private interface Invocation {
        void run() throws ErrnoException;
    }

    private static void expectEnotsup(String shorty, Invocation invocation) throws Exception {
        try {
            invocation.run();
            throw new AssertionError(shorty + " unexpectedly succeeded");
        } catch (ErrnoException expected) {
            if (expected.errno != 95) {
                throw new AssertionError(shorty + " errno=" + expected.errno, expected);
            }
        }
    }

    private static void expectErrno(int errno, String operation, Invocation invocation)
            throws Exception {
        try {
            invocation.run();
            throw new AssertionError(operation + " unexpectedly succeeded");
        } catch (ErrnoException expected) {
            if (expected.errno != errno) {
                throw new AssertionError(operation + " errno=" + expected.errno, expected);
            }
        }
    }

    public static void main(String[] args) throws Exception {
        if (args.length != 1) {
            throw new IllegalArgumentException("native library path required");
        }
        System.load(args[0]);
        LibcoreDarwinAbiSmoke smoke = new LibcoreDarwinAbiSmoke();
        FileDescriptor fd = new FileDescriptor();
        expectEnotsup("V", () -> smoke.unsupportedVoid("v", 1));
        expectEnotsup("I", () -> smoke.unsupportedInt(fd, 2, 3));
        expectEnotsup("J", () -> smoke.unsupportedLong(fd));
        expectEnotsup("L", () -> smoke.unsupportedObject("o"));
        expectEnotsup("Z", () -> smoke.unsupportedBoolean("z", 4));
        long processors = smoke.availableProcessors();
        if (processors <= 0) {
            throw new AssertionError("availableProcessors=" + processors);
        }
        String environment = smoke.environment("DARWIN_ART_MANAGED_SMOKE");
        if (!"present".equals(environment)) {
            throw new AssertionError("getenv=" + environment);
        }
        String variable = "DARWIN_ART_MANAGED_SETENV";
        smoke.unsetEnvironment(variable);
        if (smoke.environment(variable) != null) {
            throw new AssertionError("unsetenv left a value");
        }
        try {
            smoke.setEnvironment(variable, "first", false);
            if (!"first".equals(smoke.environment(variable))) {
                throw new AssertionError("setenv did not publish first value");
            }
            smoke.setEnvironment(variable, "second", false);
            if (!"first".equals(smoke.environment(variable))) {
                throw new AssertionError("setenv overwrite=false changed value");
            }
            smoke.setEnvironment(variable, "second", true);
            if (!"second".equals(smoke.environment(variable))) {
                throw new AssertionError("setenv overwrite=true did not change value");
            }
            expectErrno(22, "setenv empty name", () ->
                    smoke.setEnvironment("", "value", true));
            expectErrno(22, "setenv equals name", () ->
                    smoke.setEnvironment("name=value", "value", true));
            expectErrno(22, "unsetenv empty name", () ->
                    smoke.unsetEnvironment(""));
            expectErrno(22, "unsetenv equals name", () ->
                    smoke.unsetEnvironment("name=value"));
        } finally {
            smoke.unsetEnvironment(variable);
        }
        try {
            smoke.setEnvironment(null, "value", true);
            throw new AssertionError("setenv(null, ...) unexpectedly succeeded");
        } catch (NullPointerException expected) {
            // ScopedUtfChars preserves the AOSP null argument contract.
        }
        try {
            smoke.setEnvironment("name", null, true);
            throw new AssertionError("setenv(..., null, ...) unexpectedly succeeded");
        } catch (NullPointerException expected) {
            // ScopedUtfChars preserves the AOSP null argument contract.
        }
        try {
            smoke.unsetEnvironment(null);
            throw new AssertionError("unsetenv(null) unexpectedly succeeded");
        } catch (NullPointerException expected) {
            // ScopedUtfChars preserves the AOSP null argument contract.
        }
        byte[] payload = "PK\u0003\u0004darwin-libcore-writePK\u0005\u0006"
                .getBytes(StandardCharsets.ISO_8859_1);
        File temporary = File.createTempFile("darwin-art-libcore-", ".tmp");
        try {
            if (!smoke.accessPath(temporary.getAbsolutePath(), 0)) {
                throw new AssertionError("access(F_OK) rejected an existing path");
            }
            expectErrno(2, "access missing", () ->
                    smoke.accessPath(temporary.getAbsolutePath() + ".missing", 0));
            expectErrno(22, "access mode", () ->
                    smoke.accessPath(temporary.getAbsolutePath(), 8));
            int written = smoke.writeFile(temporary.getAbsolutePath(), payload);
            StructStat status = smoke.statPath(temporary.getAbsolutePath());
            if (written != payload.length || status == null || status.st_size != payload.length) {
                throw new AssertionError("write/stat roundtrip failed");
            }
            FileDescriptor randomAccess = smoke.openFile(temporary.getAbsolutePath(), 0, 0);
            try {
                if (smoke.seekFile(randomAccess, 4L, 0) != 4L) {
                    throw new AssertionError("SEEK_SET failed");
                }
                byte[] middle = new byte[6];
                if (smoke.readFile(randomAccess, middle, 0, middle.length) != middle.length
                        || !"darwin".equals(new String(
                                middle, StandardCharsets.ISO_8859_1))) {
                    throw new AssertionError("RandomAccessFile-style seek/read failed");
                }
                if (smoke.seekFile(randomAccess, -4L, 2) != payload.length - 4L) {
                    throw new AssertionError("SEEK_END failed");
                }
                byte[] endRecord = new byte[4];
                if (smoke.readFile(randomAccess, endRecord, 0, endRecord.length)
                                != endRecord.length
                        || endRecord[0] != 'P' || endRecord[1] != 'K'
                        || endRecord[2] != 5 || endRecord[3] != 6) {
                    throw new AssertionError("zip end-record seek failed");
                }
                if (smoke.seekFile(randomAccess, -2L, 1) != payload.length - 2L) {
                    throw new AssertionError("SEEK_CUR failed");
                }
                if (smoke.seekFile(randomAccess, Long.MAX_VALUE, 0) != Long.MAX_VALUE) {
                    throw new AssertionError("64-bit offset narrowed");
                }
                expectErrno(75, "lseek overflow",
                        () -> smoke.seekFile(randomAccess, 1L, 1));
                expectErrno(22, "lseek whence",
                        () -> smoke.seekFile(randomAccess, 0L, 99));
            } finally {
                smoke.closeFile(randomAccess);
            }
        } finally {
            temporary.delete();
        }
        StructUtsname uname = smoke.unameView();
        if (uname == null || !"Linux".equals(uname.sysname)
                || !"aarch64".equals(uname.machine)) {
            throw new AssertionError("uname compatibility projection failed");
        }
        String invalidArgument = smoke.errorMessage(22);
        if (invalidArgument == null || invalidArgument.isEmpty()) {
            throw new AssertionError("strerror(EINVAL)=" + invalidArgument);
        }
        String terminationSignal = smoke.signalMessage(15);
        if (terminationSignal == null || terminationSignal.isEmpty()) {
            throw new AssertionError("strsignal(SIGTERM)=" + terminationSignal);
        }
        exchangeOwnerTag(fd, 1L, 2L);
        if (getOwnerTag(fd) != 0L || getTagValue(2L) != 0L
                || !"unknown".equals(getTagType(2L))) {
            throw new AssertionError("non-Bionic fdsan contract failed");
        }
        System.out.println("managed-abi: V/I/J/L/Z ErrnoException(ENOTSUP)"
                + " env=getenv+setenv/overwrite/null/einval+unsetenv/null/einval"
                + " access=existing/missing/mode lseek=set/cur/end+zip+overflow strerror(EINVAL)=pass"
                + " strsignal(SIGTERM)=pass fdsan=non-Bionic"
                + " processors=" + processors);
    }
}
JAVA
javac --release 17 -encoding UTF-8 -d "$java_classes" \
  "$java_sources/android/system/StructTimespec.java" \
  "$java_sources/android/system/StructStat.java" \
  "$java_sources/android/system/StructUtsname.java" \
  "$java_sources/android/system/ErrnoException.java" \
  "$java_sources/dev/darwinart/probe/LibcoreDarwinAbiSmoke.java"
managed_abi_output="$(DARWIN_ART_MANAGED_SMOKE=present java -cp "$java_classes" \
  dev.darwinart.probe.LibcoreDarwinAbiSmoke "$abi_library")"
[[ "$managed_abi_output" == \
   'managed-abi: V/I/J/L/Z ErrnoException(ENOTSUP) env=getenv+setenv/overwrite/null/einval+unsetenv/null/einval access=existing/missing/mode lseek=set/cur/end+zip+overflow strerror(EINVAL)=pass strsignal(SIGTERM)=pass fdsan=non-Bionic processors='* ]] ||
  fail "managed ABI smoke output mismatch: $managed_abi_output"
managed_processors="${managed_abi_output##*=}"
[[ "$managed_processors" =~ ^[1-9][0-9]*$ ]] ||
  fail "managed availableProcessors is not positive: $managed_processors"


build_dir="$project_root/_build/libcore-darwin-linux"
mkdir -p "$build_dir"
cp "$smoke" "$build_dir/libcore-darwin-linux-smoke"
echo "libcore-darwin-linux-test: archive=provided native=PASS managed=PASS $smoke_output"
