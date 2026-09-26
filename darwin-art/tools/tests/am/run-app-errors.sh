#!/bin/bash
set -euo pipefail
root="$(cd "$(dirname "$0")/../../.." && pwd)"
test_dir="$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-app-errors.XXXXXX")"
trap 'rm -rf "$test_dir"' EXIT
android_jar="${ANDROID_PLATFORM_JAR:-$HOME/Library/Android/sdk/platforms/android-36/android.jar}"
javac -cp "$android_jar" -d "$test_dir" \
  $(find "$root/tools/tests/am/app-errors-stubs" -name '*.java' -print) \
  "$root/runtime/framework/am/ApplicationProcessRegistry.java" \
  "$root/runtime/framework/am/AppErrors.java" \
  "$root/tools/tests/am/AppErrorsTest.java"
java -cp "$test_dir:$android_jar" dev.darwinart.runtime.am.AppErrorsTest
