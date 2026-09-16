#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/../../.." && pwd)"
java_home="${JAVA_HOME:-/opt/homebrew/opt/openjdk@17}"
jni_include="$java_home/include"
jni_darwin_include="$jni_include/darwin"

[[ -f "$jni_include/jni.h" ]] || {
  echo "OpenJDK JNI headers are missing: $jni_include/jni.h" >&2
  exit 69
}

tmp="$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-installed-record-source.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/classes"

javac --release 8 -encoding UTF-8 -d "$tmp/classes" \
  "$root/tools/tests/installed-record-source/InstalledRecordSourceTest.java"

clang++ -std=c++20 -dynamiclib -fPIC -Wall -Wextra -Werror \
  -I"$jni_include" -I"$jni_darwin_include" \
  "$root/runtime/framework/pm/installed_record_source.cc" \
  "$root/tools/tests/installed-record-source/fixture.cc" \
  -o "$tmp/libinstalled_record_source_test.dylib"

java -ea -cp "$tmp/classes" InstalledRecordSourceTest \
  "$tmp/libinstalled_record_source_test.dylib"
echo "installed-record-source: PASS (isolated production JNI + test-only FFI)"
