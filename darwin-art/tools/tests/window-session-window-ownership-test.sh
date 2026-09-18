#!/bin/bash
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
stage="$(mktemp -d "${TMPDIR:-/tmp}/window-session-window-ownership.XXXXXX")"
trap 'rm -rf "$stage"' EXIT
android_jar="${ANDROID_PLATFORM_JAR:-$HOME/Library/Android/sdk/platforms/android-36/android.jar}"
javac --release 8 -cp "$android_jar" -encoding UTF-8 -d "$stage" \
  "$root/runtime/framework/wm/WindowSessionWindowOwnership.java" \
  "$root/tools/tests/wm/WindowSessionWindowOwnershipTest.java"
java -cp "$android_jar:$stage" dev.darwinart.runtime.wm.WindowSessionWindowOwnershipTest
