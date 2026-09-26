#!/bin/bash
set -euo pipefail

mode="${1:---archive-and-test}"
[[ $# -le 1 && ( "$mode" == --archive-only || "$mode" == --archive-and-test ) ]] || {
  echo 'usage: build-android16-libcore-darwin-linux.sh [--archive-only|--archive-and-test]' >&2
  exit 2
}

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
  mkdir => 'DarwinLinuxMkdir', lstat => 'DarwinLinuxLstat',
  unlink => 'DarwinLinuxUnlink', readlink => 'DarwinLinuxReadlink',
  symlink => 'DarwinLinuxSymlink', link => 'DarwinLinuxLink',
  fsync => 'DarwinLinuxFsync', fdatasync => 'DarwinLinuxFdatasync',
  posix_fallocate => 'DarwinLinuxPosixFallocate',
  getxattr => 'DarwinLinuxGetxattr', setxattr => 'DarwinLinuxSetxattr',
  removexattr => 'DarwinLinuxRemovexattr', listxattr => 'DarwinLinuxListxattr',
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
"$script_dir/build-android16-asynchronous-close-monitor.sh" --archive-only >/dev/null
"$script_dir/build-android16-os-constants-darwin.sh" --archive-only >/dev/null
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



build_dir="$project_root/_build/libcore-darwin-linux"
mkdir -p "$build_dir"
cp "$archive" "$build_dir/libcore-darwin-linux.a"
cp "$method_include" "$build_dir/darwin_linux_method_table.inc"
cp "$manifest" "$build_dir/libcore-darwin-linux-methods.tsv"
cp "$abi_manifest" "$build_dir/libcore-darwin-linux-unsupported-abi.tsv"
cp "$counts" "$build_dir/libcore-darwin-linux-counts.txt"
cp "$definitions" "$build_dir/libcore-darwin-linux-definitions.txt"
echo "libcore-darwin-linux: archive=Mach-O-arm64 methods=$method_count regular=$supported_regular critical=$supported_critical enotsup=$unsupported_count"
if [[ "$mode" == --archive-and-test ]]; then
  bash "$script_dir/test-android16-libcore-darwin-linux.sh" --prepared
fi
