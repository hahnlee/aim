#!/bin/bash
set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/.." && pwd)"
lock_file="$project_root/upstream/android16-libcore-darwin-linux.lock"
# shellcheck disable=SC1090
source "$lock_file"

sha256() { shasum -a 256 "$1" | awk '{print $1}'; }
fail() { echo "libcore-darwin-linux: $*" >&2; exit 3; }

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

wrappers="$stage/wrappers.inc"
entries="$stage/entries.inc"
abi_manifest="$stage/unsupported-abi.tsv"
counts="$stage/counts.txt"
perl - "$manifest" "$wrappers" "$entries" "$abi_manifest" "$counts" <<'PERL'
use strict;
use warnings;

my ($manifest, $wrappers, $entries, $abi_manifest, $counts) = @ARGV;
open my $input, '<', $manifest or die "open manifest: $!";
open my $wrapper_out, '>', $wrappers or die "open wrappers: $!";
open my $entry_out, '>', $entries or die "open entries: $!";
open my $abi_out, '>', $abi_manifest or die "open ABI manifest: $!";

my %primitive = (
  V => 'void', Z => 'jboolean', B => 'jbyte', C => 'jchar',
  S => 'jshort', I => 'jint', J => 'jlong', F => 'jfloat', D => 'jdouble',
);
my %primitive_array = (
  Z => 'jbooleanArray', B => 'jbyteArray', C => 'jcharArray',
  S => 'jshortArray', I => 'jintArray', J => 'jlongArray',
  F => 'jfloatArray', D => 'jdoubleArray',
);

sub parse_type {
  my ($descriptor, $position_ref, $allow_void) = @_;
  my $position = $$position_ref;
  my $kind = substr($descriptor, $position, 1);
  die "truncated descriptor: $descriptor" if $kind eq '';
  if ($kind eq '[') {
    my $dimensions = 0;
    while (substr($descriptor, $position, 1) eq '[') {
      ++$dimensions;
      ++$position;
    }
    my $element = substr($descriptor, $position, 1);
    if ($element eq 'L') {
      my $end = index($descriptor, ';', $position);
      die "unterminated object array: $descriptor" if $end < 0;
      $position = $end + 1;
      $$position_ref = $position;
      return 'jobjectArray';
    }
    die "bad array element: $descriptor" unless exists $primitive_array{$element};
    ++$position;
    $$position_ref = $position;
    return $dimensions == 1 ? $primitive_array{$element} : 'jobjectArray';
  }
  if ($kind eq 'L') {
    my $end = index($descriptor, ';', $position);
    die "unterminated object: $descriptor" if $end < 0;
    my $class_name = substr($descriptor, $position + 1, $end - $position - 1);
    $$position_ref = $end + 1;
    return 'jstring' if $class_name eq 'java/lang/String';
    return 'jclass' if $class_name eq 'java/lang/Class';
    return 'jthrowable' if $class_name eq 'java/lang/Throwable';
    return 'jobject';
  }
  die "void parameter: $descriptor" if $kind eq 'V' && !$allow_void;
  die "unknown descriptor kind $kind: $descriptor" unless exists $primitive{$kind};
  $$position_ref = $position + 1;
  return $primitive{$kind};
}

sub decode_signature {
  my ($signature) = @_;
  die "bad signature: $signature" unless substr($signature, 0, 1) eq '(';
  my $position = 1;
  my @parameters;
  while (substr($signature, $position, 1) ne ')') {
    push @parameters, parse_type($signature, \$position, 0);
  }
  ++$position;
  my $return_type = parse_type($signature, \$position, 1);
  die "trailing descriptor data: $signature" unless $position == length($signature);
  return ($return_type, @parameters);
}

my %critical = (
  nativeGetegid => 'DarwinNativeGetegid', nativeGeteuid => 'DarwinNativeGeteuid',
  nativeGetgid => 'DarwinNativeGetgid', nativeGetpid => 'DarwinNativeGetpid',
  nativeGetppid => 'DarwinNativeGetppid', nativeGettid => 'DarwinNativeGettid',
  nativeGetuid => 'DarwinNativeGetuid',
);
my %supported = (
  access => 'DarwinLinuxAccess', open => 'DarwinLinuxOpen', dup => 'DarwinLinuxDup',
  dup2 => 'DarwinLinuxDup2',
  socketpair => 'DarwinLinuxSocketpair',
  fcntlInt => 'DarwinLinuxFcntlInt', fcntlVoid => 'DarwinLinuxFcntlVoid',
  fstat => 'DarwinLinuxFstat',
  ftruncate => 'DarwinLinuxFtruncate',
  readBytes => 'DarwinLinuxReadBytes', preadBytes => 'DarwinLinuxPreadBytes',
  writeBytes => 'DarwinLinuxWriteBytes', pwriteBytes => 'DarwinLinuxPwriteBytes',
  close => 'DarwinLinuxClose',
  mmap => 'DarwinLinuxMmap', munmap => 'DarwinLinuxMunmap',
  sysconf => 'DarwinLinuxSysconf',
  getenv => 'DarwinLinuxGetenv', getpwuid => 'DarwinLinuxGetpwuid',
  setenv => 'DarwinLinuxSetenv', unsetenv => 'DarwinLinuxUnsetenv',
  stat => 'DarwinLinuxStat', lseek => 'DarwinLinuxLseek',
  kill => 'DarwinLinuxKill',
  sendfile => 'DarwinLinuxSendfile',
  remove => 'DarwinLinuxRemove', rename => 'DarwinLinuxRename',
  statvfs => 'DarwinLinuxStatvfs', fstatvfs => 'DarwinLinuxFstatvfs',
  chmod => 'DarwinLinuxChmod', fchmod => 'DarwinLinuxFchmod',
  uname => 'DarwinLinuxUname',
  strerror => 'DarwinLinuxStrerror', strsignal => 'DarwinLinuxStrsignal',
  android_fdsan_exchange_owner_tag => 'DarwinLinuxFdsanExchangeOwnerTag',
  android_fdsan_get_owner_tag => 'DarwinLinuxFdsanGetOwnerTag',
  android_fdsan_get_tag_type => 'DarwinLinuxFdsanGetTagType',
  android_fdsan_get_tag_value => 'DarwinLinuxFdsanGetTagValue',
  android_getaddrinfo => 'DarwinLinuxAndroidGetaddrinfo',
  gai_strerror => 'DarwinLinuxGaiStrerror',
  socket => 'DarwinLinuxSocket',
  connect => 'DarwinLinuxConnect',
  connectSocketAddress => 'DarwinLinuxConnectSocketAddress',
  bind => 'DarwinLinuxBind',
  bindSocketAddress => 'DarwinLinuxBindSocketAddress',
  getsockname => 'DarwinLinuxGetsockname',
  getsockoptInt => 'DarwinLinuxGetsockoptInt',
  setsockoptInt => 'DarwinLinuxSetsockoptInt',
  getsockoptTimeval => 'DarwinLinuxGetsockoptTimeval',
  setsockoptTimeval => 'DarwinLinuxSetsockoptTimeval',
  poll => 'DarwinLinuxPoll',
  shutdown => 'DarwinLinuxShutdown',
);
my ($index, $regular_count, $critical_count, $unsupported_count) = (0, 0, 0, 0);
while (my $line = <$input>) {
  chomp $line;
  my ($kind, $name, $signature) = split /\t/, $line, 3;
  ++$index;
  my $symbol;
  if ($kind eq 'CRITICAL_NATIVE_METHOD') {
    die "unowned CriticalNative method: $name" unless exists $critical{$name};
    $symbol = $critical{$name};
    ++$critical_count;
  } elsif (exists $supported{$name}) {
    my $key = $name;
    $key .= 'SocketAddress' if ($name eq 'connect' || $name eq 'bind')
        && $signature =~ /Ljava\/net\/SocketAddress;/;
    $symbol = $supported{$key};
    ++$regular_count;
  } else {
    my ($return_type, @parameters) = decode_signature($signature);
    $symbol = "DarwinUnsupported_$index";
    my @native_parameters = ('JNIEnv* env', 'jobject', @parameters);
    my $parameter_list = join(', ', @native_parameters);
    my $body = "DarwinUnsupported(env, \"libcore.io.Linux.$name\");";
    if ($return_type eq 'jobject' || $return_type eq 'jstring' ||
        $return_type eq 'jclass' || $return_type eq 'jthrowable' ||
        $return_type =~ /Array$/) {
      $body .= ' return nullptr;';
    } elsif ($return_type eq 'jfloat') {
      $body .= ' return 0.0f;';
    } elsif ($return_type eq 'jdouble') {
      $body .= ' return 0.0;';
    } elsif ($return_type ne 'void') {
      $body .= ' return 0;';
    }
    print {$wrapper_out} "$return_type $symbol($parameter_list) { $body }\n";
    print {$abi_out} join("\t", $index, $name, $signature, $return_type,
                          join(',', @native_parameters)), "\n";
    ++$unsupported_count;
  }
  print {$entry_out}
      "    {const_cast<char*>(\"$name\"), const_cast<char*>(\"$signature\"), " .
      "reinterpret_cast<void*>(&${symbol})},\n";
}
close $input;
close $wrapper_out;
close $entry_out;
close $abi_out;
open my $count_out, '>', $counts or die "open counts: $!";
print {$count_out} "regular=$regular_count\ncritical=$critical_count\n" .
                   "unsupported=$unsupported_count\n";
close $count_out;
PERL
# shellcheck disable=SC1090
source "$counts"
supported_regular="$regular"
supported_critical="$critical"
unsupported_count="$unsupported"
[[ "$supported_regular" == "$LIBCORE_DARWIN_SUPPORTED_REGULAR" ]] ||
  fail "supported regular count=$supported_regular"
[[ "$supported_critical" == "$LIBCORE_DARWIN_SUPPORTED_CRITICAL" ]] ||
  fail "supported critical count=$supported_critical"
[[ "$unsupported_count" == "$((method_count - supported_regular - supported_critical))" ]] ||
  fail "unsupported method count=$unsupported_count"
if grep -F '...' "$wrappers" >/dev/null; then
  fail "generated wrapper contains a variadic parameter"
fi
for expected in \
  $'getsockoptByte\t(Ljava/io/FileDescriptor;II)I\tjint\tJNIEnv* env,jobject,jobject,jint,jint'; do
  grep -F "$expected" "$abi_manifest" >/dev/null ||
    fail "representative fixed ABI missing: $expected"
done

method_include="$stage/darwin_linux_method_table.inc"
{
  cat "$wrappers"
  echo 'JNINativeMethod kDarwinLinuxMethods[] = {'
  cat "$entries"
  echo '};'
} > "$method_include"

cxx="$(xcrun --find clang++)"
libtool_bin="$(xcrun --find libtool)"
sdk_root="$(xcrun --sdk macosx --show-sdk-path)"
"$script_dir/build-android16-asynchronous-close-monitor.sh" >/dev/null
"$script_dir/build-android16-os-constants-darwin.sh" >/dev/null
nativehelper="$project_root/_build/nativehelper-foundation/source/libnativehelper"
nativehelper_archive="$project_root/_build/nativehelper-foundation/libnativehelper_jvm.a"
liblog_archive="$project_root/_build/graphics-foundations/liblog-darwin.a"
async_close_archive="$project_root/_build/asynchronous-close-monitor/libandroidio-darwin.a"
os_constants_archive="$project_root/_build/os-constants/libandroid-system-os-constants-darwin.a"
for required in \
  "$project_root/compat/libcore_darwin_linux.cc" \
  "$project_root/compat/libcore_darwin_linux_system_natives.cc" \
  "$project_root/compat/libcore_darwin_linux_syscalls.cc" \
  "$project_root/compat/libcore_darwin_linux.h" \
  "$project_root/compat/darwin_os_constants.h" \
  "$project_root/probes/android16_libcore_darwin_linux_smoke.cc" \
  "$nativehelper/include/nativehelper/JNIHelp.h" \
  "$nativehelper/include_platform/nativehelper/JNIPlatformHelp.h" \
  "$nativehelper/include_jni/jni.h" \
  "$nativehelper_archive" \
  "$async_close_archive" \
  "$os_constants_archive" \
  "$liblog_archive"; do
  [[ -e "$required" ]] || { echo "libcore-darwin-linux: missing $required" >&2; exit 2; }
done

common_flags=(
  -std=c++20 -arch arm64 -isysroot "$sdk_root" -fPIC -Wall -Wextra -Werror
  -I"$project_root/compat"
  -I"$project_root/tools/bionic-socket-broker-adapter/include"
  -I"$project_root/tools/bionic-process-state-facade/include"
  -I"$project_root/tools/bionic-fs-facade/include"
  -I"$project_root/tools/bionic-ioctl-facade/include"
  -I"$stage"
  -I"$nativehelper/include_jni"
  -I"$nativehelper/include"
  -I"$nativehelper/include_platform"
  -I"$nativehelper/header_only_include"
  -I"$project_root/_aosp/system/logging/liblog/include"
)
object="$stage/libcore_darwin_linux.o"
"$cxx" "${common_flags[@]}" -c \
  "$project_root/compat/libcore_darwin_linux.cc" -o "$object"
[[ "$(file "$object")" == *"Mach-O 64-bit object arm64"* ]] ||
  fail "JNI registrar object is not Mach-O arm64"

syscalls_object="$stage/libcore_darwin_linux_syscalls.o"
"$cxx" "${common_flags[@]}" -c \
  "$project_root/compat/libcore_darwin_linux_syscalls.cc" -o "$syscalls_object"
[[ "$(file "$syscalls_object")" == *"Mach-O 64-bit object arm64"* ]] ||
  fail "syscall backend object is not Mach-O arm64"

system_object="$stage/libcore_darwin_linux_system_natives.o"
"$cxx" "${common_flags[@]}" -c \
  "$project_root/compat/libcore_darwin_linux_system_natives.cc" -o "$system_object"
[[ "$(file "$system_object")" == *"Mach-O 64-bit object arm64"* ]] ||
  fail "system/metadata JNI object is not Mach-O arm64"

archive="$stage/libcore-darwin-linux.a"
"$libtool_bin" -static -o "$archive" "$object" "$system_object" "$syscalls_object"
definitions="$stage/definitions.txt"
nm -gU "$archive" | c++filt > "$definitions"
grep -F ' T darwin_art::libcore_darwin::RegisterLinuxNatives(_JNIEnv*)' \
  "$definitions" >/dev/null || fail "registrar definition missing"

smoke="$stage/libcore-darwin-linux-smoke"
"$cxx" "${common_flags[@]}" \
  "$project_root/probes/android16_libcore_darwin_linux_smoke.cc" \
  "$object" "$system_object" "$syscalls_object" "$async_close_archive" "$os_constants_archive" \
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
cp "$archive" "$build_dir/libcore-darwin-linux.a"
cp "$smoke" "$build_dir/libcore-darwin-linux-smoke"

echo "libcore-darwin-linux: methods=$method_count regular=$supported_regular critical=$supported_critical enotsup=$unsupported_count fixed-abi=$unsupported_count managed=V/I/J/L/Z+getenv/setenv/unsetenv/access/stat/write/lseek/uname/strerror/strsignal/fdsan managed-processors=$managed_processors archive=Mach-O-arm64 $smoke_output"
