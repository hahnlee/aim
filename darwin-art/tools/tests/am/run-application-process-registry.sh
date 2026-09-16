#!/bin/bash
set -euo pipefail
root="$(cd "$(dirname "$0")/../../.." && pwd)"
test_dir="$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-am-process-registry.XXXXXX")"
trap 'rm -rf "$test_dir"' EXIT
android_jar="${ANDROID_PLATFORM_JAR:-$HOME/Library/Android/sdk/platforms/android-36/android.jar}"
javac -cp "$android_jar" -d "$test_dir" \
  "$root/runtime/framework/am/ApplicationProcessRegistry.java" \
  "$root/tools/tests/am/ApplicationProcessRegistryTest.java"
java -cp "$android_jar:$test_dir" ApplicationProcessRegistryTest
