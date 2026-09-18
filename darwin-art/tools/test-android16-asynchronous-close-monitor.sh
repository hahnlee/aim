#!/bin/bash
set -euo pipefail
export LC_ALL=C

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/.." && pwd)"
if [[ $# == 0 ]]; then
  bash "$script_dir/build-android16-asynchronous-close-monitor.sh" --archive-only
elif [[ $# != 1 || "$1" != --prepared ]]; then
  echo 'usage: test-android16-asynchronous-close-monitor.sh [--prepared]' >&2
  exit 2
fi

lock_file="$project_root/upstream/android16-asynchronous-close-monitor.lock"
source_root="$project_root/_aosp/libcore-asynchronous-close"
build_dir="$project_root/_build/asynchronous-close-monitor"

# shellcheck disable=SC1090
source "$lock_file"

sha256() { shasum -a 256 "$1" | awk '{print $1}'; }
fail() { echo "async-close-test: $*" >&2; exit 3; }

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

materialize luni/src/main/java/libcore/io/AsynchronousCloseMonitor.java \
  "$MANAGED_CLASS_SHA256"
materialize luni/src/main/java/libcore/io/IoBridge.java "$IO_BRIDGE_SHA256"

nativehelper="$project_root/_aosp/libnativehelper-full"
nativehelper_archive="$project_root/_build/nativehelper-foundation/libnativehelper_jvm.a"
liblog_archive="$project_root/_build/graphics-foundations/liblog-darwin.a"
archive="$build_dir/libandroidio-darwin.a"
registrar_archive="$build_dir/libcore-io-asynchronous-close-monitor-registrar-darwin.a"
native_bp="$source_root/luni/src/main/native/Android.bp"
register_source="$source_root/luni/src/main/native/Register.cpp"
registrar_source="$source_root/luni/src/main/native/libcore_io_AsynchronousCloseMonitor.cpp"
io_bridge="$source_root/luni/src/main/java/libcore/io/IoBridge.java"

for required in \
  "$archive" "$registrar_archive" "$project_root/compat/AsynchronousCloseMonitor.h" \
  "$project_root/compat/darwin_asynchronous_close_monitor.cc" \
  "$project_root/probes/android16_asynchronous_close_monitor_smoke.cc" \
  "$project_root/probes/android16_asynchronous_close_monitor_jni.cc" \
  "$nativehelper/include/nativehelper/JNIHelp.h" \
  "$nativehelper/include_platform/nativehelper/JNIPlatformHelp.h" \
  "$nativehelper/include_platform_header_only/nativehelper/jni_macros.h" \
  "$nativehelper/include_jni/jni.h" "$nativehelper_archive" "$liblog_archive"; do
  [[ -f "$required" ]] || fail "missing test dependency: $required"
done

grep -F 'AsynchronousCloseMonitor.signalBlockedThreads(oldFd);' \
  "$io_bridge" >/dev/null || fail "IoBridge signal-before-close contract missing"
method_count="$(grep -c 'NATIVE_METHOD(AsynchronousCloseMonitor,' \
  "$registrar_source")"
[[ "$method_count" == "$REGISTRAR_METHOD_COUNT" ]] ||
  fail "registrar method count=$method_count"
grep -F 'signalBlockedThreads, "(Ljava/io/FileDescriptor;)V"' \
  "$registrar_source" >/dev/null || fail "exact managed JNI signature missing"

stage="$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-async-close-test.XXXXXX")"
trap 'rm -rf "$stage"' EXIT

cxx="$(xcrun --find clang++)"
sdk_root="$(xcrun --sdk macosx --show-sdk-path)"
common_flags=(
  -std=c++20 -arch arm64 -isysroot "$sdk_root" -fPIC
  -Wall -Wextra -Werror
  -I"$project_root/compat"
  -I"$nativehelper/include_jni"
  -I"$nativehelper/include"
  -I"$nativehelper/include_platform"
  -I"$nativehelper/include_platform_header_only"
  -I"$nativehelper/header_only_include"
  -I"$project_root/_aosp/system/logging/liblog/include"
)

smoke="$stage/asynchronous-close-monitor-smoke"
"$cxx" "${common_flags[@]}" \
  "$project_root/probes/android16_asynchronous_close_monitor_smoke.cc" \
  -Wl,-force_load,"$archive" "$liblog_archive" -o "$smoke"
smoke_output="$($smoke)"
[[ "$smoke_output" == 'async-close: two-blocked-readers=EINTR signaled=2' ]] ||
  fail "blocking smoke failed: $smoke_output"

jni_object="$stage/asynchronous-close-monitor-jni.o"
"$cxx" "${common_flags[@]}" \
  -c "$project_root/probes/android16_asynchronous_close_monitor_jni.cc" \
  -o "$jni_object"
managed_library="$stage/libasynchronous-close-monitor-managed.dylib"
"$cxx" -arch arm64 -isysroot "$sdk_root" -dynamiclib \
  "$jni_object" -Wl,-force_load,"$registrar_archive" \
  -Wl,-force_load,"$archive" \
  -Wl,-force_load,"$nativehelper_archive" "$liblog_archive" \
  -o "$managed_library"

java_sources="$stage/java-sources"
java_classes="$stage/java-classes"
mkdir -p "$java_sources/libcore/io" "$java_sources/dev/darwinart/probe" \
  "$java_classes"
cat > "$java_sources/libcore/io/AsynchronousCloseMonitor.java" <<'JAVA'
package libcore.io;

import java.io.FileDescriptor;

public final class AsynchronousCloseMonitor {
    private AsynchronousCloseMonitor() {}
    public static native void signalBlockedThreads(FileDescriptor fd);
}
JAVA
cat > "$java_sources/dev/darwinart/probe/AsynchronousCloseMonitorSmoke.java" <<'JAVA'
package dev.darwinart.probe;

import java.io.FileInputStream;
import libcore.io.AsynchronousCloseMonitor;

public final class AsynchronousCloseMonitorSmoke {
    public static void main(String[] args) throws Exception {
        if (args.length != 1) throw new IllegalArgumentException("dylib required");
        System.load(args[0]);
        try (FileInputStream input = new FileInputStream("/dev/null")) {
            AsynchronousCloseMonitor.signalBlockedThreads(input.getFD());
        }
        System.out.println("managed-async-close: registrar=pass signature=FileDescriptor->void");
    }
}
JAVA
javac --release 17 -encoding UTF-8 -d "$java_classes" \
  "$java_sources/libcore/io/AsynchronousCloseMonitor.java" \
  "$java_sources/dev/darwinart/probe/AsynchronousCloseMonitorSmoke.java"
managed_output="$(java -cp "$java_classes" \
  dev.darwinart.probe.AsynchronousCloseMonitorSmoke "$managed_library")"
[[ "$managed_output" == \
   'managed-async-close: registrar=pass signature=FileDescriptor->void' ]] ||
  fail "managed registrar smoke failed: $managed_output"

retained_undefined="$stage/managed-retained-undefined.txt"
nm -u "$managed_library" | sed 's/^[[:space:]]*//' | sort -u > "$retained_undefined"
if grep -F '_IO_fd_fdID' "$retained_undefined" >/dev/null; then
  fail "managed test retained unrelated FileDescriptor state"
fi
mkdir -p "$build_dir"
cp "$retained_undefined" "$build_dir/managed-retained-undefined.txt"
echo "async-close: signal=SIGUSR2 blocked-readers=2/EINTR managed=pass"
