#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/../../.." && pwd)"
java_home="${JAVA_HOME:-/opt/homebrew/opt/openjdk@17}"
android_jar="${ANDROID_PLATFORM_JAR:-$HOME/Library/Android/sdk/platforms/android-36/android.jar}"
tmp="$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-storage-endpoint.XXXXXX")"
mkdir -p "$tmp/classes"
# The endpoint uses non-SDK framework types (VolumeInfo, StorageManager
# internals); compile against the platform signatures the runtime build
# generated.
signatures=""
for candidate in "$root"/_build/runtime-support-java/generation.*/compile-signatures; do
  [[ -z "$signatures" || "$candidate" -nt "$signatures" ]] && signatures="$candidate"
done
[[ -d "$signatures" ]] || { echo "run cargo xtask build first (no platform signatures)" >&2; exit 69; }

"$java_home/bin/javac" --release 8 -encoding UTF-8 -d "$tmp/classes" \
  -classpath "$signatures" \
  "$root/runtime/framework/am/ApplicationProcessRegistry.java" \
  "$root/runtime/framework/storage/StorageManagerEndpoint.java" \
  "$root/runtime/framework/storage/UserStorage.java" \
  "$root/runtime/framework/storage/InternalVolume.java" \
  "$root/tools/tests/storage/StorageManagerEndpointTest.java"
"$java_home/bin/java" -ea -cp "$tmp/classes:$android_jar" \
  dev.darwinart.runtime.storage.StorageManagerEndpointTest
