#!/bin/bash
set -euo pipefail
root="$(cd "$(dirname "$0")/../../.." && pwd)"
test_dir="$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-incoming-users.XXXXXX")"
trap 'rm -rf "$test_dir"' EXIT
android_jar="${ANDROID_PLATFORM_JAR:-$HOME/Library/Android/sdk/platforms/android-36/android.jar}"
javac -cp "$android_jar" -d "$test_dir" \
  $(find "$root/tools/tests/am/incoming-users-stubs" -name '*.java' -print) \
  "$root/runtime/framework/am/IncomingUsers.java" \
  "$root/tools/tests/am/IncomingUsersTest.java"
java -cp "$test_dir:$android_jar" dev.darwinart.runtime.am.IncomingUsersTest
