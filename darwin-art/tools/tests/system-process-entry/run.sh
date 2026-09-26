#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "$0")/../../.." && pwd)"
java_root="${JAVA_HOME:-/opt/homebrew/opt/openjdk@17}"
stage="$(mktemp -d /tmp/darwin-art-system-entry.XXXXXX)"
trap 'rm -rf "$stage"' EXIT
"$java_root/bin/javac" -d "$stage/classes" "$root"/tools/tests/system-process-entry/*.java
clang++ -std=c++20 -dynamiclib -Wall -Wextra -Werror \
  -I"$java_root/include" -I"$java_root/include/darwin" -I"$root/compat" \
  "$root/runtime/framework/system/process_entry.cc" \
  "$root/runtime/framework/app/java_exception_report.cc" \
  "$root/tools/tests/system-process-entry/fixture.cc" -o "$stage/entry.dylib"
# The image's derive_classpath environment file.
image="$stage/image with spaces"
mkdir -p "$image/system/etc"
printf '%s\n' \
  'export SYSTEMSERVERCLASSPATH /system/framework/services.jar:/apex/com.android.art/javalib/service-art.jar' \
  'export STANDALONE_SYSTEMSERVER_JARS /apex/com.android.os.statsd/javalib/service-statsd.jar' \
  >"$image/system/etc/classpath"
DARWIN_ART_TEST_IMAGE="$image" DARWIN_ART_ANDROID_FILESYSTEM_ROOT="$image" \
  "$java_root/bin/java" -ea -cp "$stage/classes" SystemProcessEntryTest "$stage/entry.dylib"
