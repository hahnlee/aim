#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/../../.." && pwd)"
java_home="${JAVA_HOME:-/opt/homebrew/opt/openjdk@17}"
android_jar="${ANDROID_PLATFORM_JAR:-$HOME/Library/Android/sdk/platforms/android-36/android.jar}"
out="$(mktemp -d "${TMPDIR:-/tmp}/broadcast-test.XXXXXX")"
trap 'rm -rf -- "$out"' EXIT
mkdir -p "$out/classes"
"$java_home/bin/javac" --release 8 -encoding UTF-8 -cp "$android_jar" -d "$out/classes" \
  $(find "$root/tools/tests/am/stubs" "$root/tools/tests/broadcast/stubs" -name '*.java' -print) \
  "$root/probes/compile-stubs/android/content/res/CompatibilityInfo.java" \
  "$root/runtime/framework/am/ApplicationProcessRegistry.java" \
  "$root/runtime/framework/am/BroadcastRegistry.java" \
  "$root/runtime/framework/am/SystemBroadcasts.java" \
  "$root/runtime/framework/power/BatteryStateProvider.java" \
  "$root/runtime/framework/power/BatteryHealth.java" \
  "$root/runtime/framework/power/BatteryService.java" \
  "$root/tools/tests/broadcast/BroadcastRegistryTest.java" \
  "$root/tools/tests/broadcast/BatteryServiceTest.java"
"$java_home/bin/java" -cp "$out/classes:$android_jar" dev.darwinart.runtime.am.BroadcastRegistryTest
"$java_home/bin/java" -cp "$out/classes:$android_jar" dev.darwinart.runtime.power.BatteryServiceTest
