#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/../../.." && pwd)"
java_home="${JAVA_HOME:-/opt/homebrew/opt/openjdk@17}"
android_jar="${ANDROID_PLATFORM_JAR:-$HOME/Library/Android/sdk/platforms/android-36/android.jar}"
tmp="$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-storage-endpoint.XXXXXX")"
mkdir -p "$tmp/classes"

"$java_home/bin/javac" --release 8 -encoding UTF-8 -d "$tmp/classes" \
  -classpath "$android_jar" \
  "$root/runtime/framework/am/ApplicationProcessRegistry.java" \
  "$root/runtime/framework/storage/StorageManagerEndpoint.java" \
  "$root/tools/tests/storage/StorageManagerEndpointTest.java"
"$java_home/bin/java" -ea -cp "$tmp/classes:$android_jar" \
  dev.darwinart.runtime.storage.StorageManagerEndpointTest
