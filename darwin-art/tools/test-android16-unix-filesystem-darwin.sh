#!/bin/bash
set -euo pipefail
export LC_ALL=C
script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/.." && pwd)"
if [[ $# == 0 ]]; then
  bash "$script_dir/build-android16-unix-filesystem-darwin.sh" --archive-only
elif [[ $# != 1 || "$1" != --prepared ]]; then
  echo 'usage: test-android16-unix-filesystem-darwin.sh [--prepared]' >&2
  exit 2
fi
build_dir="$project_root/_build/unix-filesystem-darwin"
archive="$build_dir/libopenjdk-unix-filesystem-darwin.a"
nativehelper_source="$project_root/_aosp/libnativehelper-full"
nativehelper_archive="$project_root/_build/nativehelper-device-foundation/libnativehelper-device-darwin.a"
liblog_archive="$project_root/_build/graphics-foundations/liblog-darwin.a"
for required in "$archive" "$nativehelper_archive" "$liblog_archive" \
  "$project_root/probes/android16_unix_filesystem_jni.c" \
  "$project_root/probes/unix-filesystem/UnixFileSystemDarwinSmoke.java"; do
  [[ -f "$required" ]] || { echo "unix-filesystem-test: missing $required" >&2; exit 3; }
done
stage="$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-unixfs-test.XXXXXX")"
trap 'rm -rf "$stage"' EXIT
cc="$(xcrun --find clang)"
sdk_root="$(xcrun --sdk macosx --show-sdk-path)"
probe_object="$stage/android16_unix_filesystem_jni.o"
"$cc" -std=gnu11 -arch arm64 -isysroot "$sdk_root" -fPIC \
  -Wall -Wextra -Werror -I"$nativehelper_source/include_jni" \
  -c "$project_root/probes/android16_unix_filesystem_jni.c" -o "$probe_object"
managed_library="$stage/libunix-filesystem-darwin-managed.dylib"
"$cc" -arch arm64 -isysroot "$sdk_root" -dynamiclib \
  "$probe_object" -Wl,-force_load,"$archive" \
  -Wl,-force_load,"$nativehelper_archive" "$liblog_archive" \
  -Wl,-exported_symbol,_JNI_OnLoad -Wl,-dead_strip \
  -Wl,-undefined,dynamic_lookup -framework CoreFoundation -o "$managed_library"
retained_undefined="$stage/managed-retained-undefined.txt"
nm -u "$managed_library" | sed 's/^[[:space:]]*//' | sort -u > "$retained_undefined"
if grep -F '_IO_fd_fdID' "$retained_undefined" >/dev/null; then
  echo 'unix-filesystem-test: dead-strip retained unrelated FileDescriptor state' >&2
  exit 3
fi
grep -Fx '_JVM_GetLastErrorString' "$retained_undefined" >/dev/null || {
  echo 'unix-filesystem-test: libopenjdkjvm provider contract disappeared' >&2
  exit 3
}
classes="$stage/classes"
mkdir -p "$classes"
javac --release 17 -encoding UTF-8 -d "$classes" \
  "$project_root/probes/unix-filesystem/UnixFileSystemDarwinSmoke.java"
managed_output="$(java -cp "$classes" \
  dev.darwinart.probe.UnixFileSystemDarwinSmoke "$managed_library")"
expected='managed-unixfs: methods=12 list0=alpha.txt,beta.txt canonicalize=pass attributes=pass permissions=pass space=pass'
[[ "$managed_output" == "$expected" ]] || {
  echo "unix-filesystem-test: managed acceptance failed: $managed_output" >&2
  exit 3
}
cp "$retained_undefined" "$build_dir/managed-retained-undefined.txt"
echo "$managed_output"
