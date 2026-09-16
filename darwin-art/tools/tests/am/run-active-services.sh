#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/../../.." && pwd)"
java_home="${JAVA_HOME:-/opt/homebrew/opt/openjdk@17}"
android_jar="${ANDROID_PLATFORM_JAR:-$HOME/Library/Android/sdk/platforms/android-36/android.jar}"
out="$root/_build/active-services-test"
mkdir -p "$out/classes"
"$java_home/bin/javac" --release 8 -encoding UTF-8 -cp "$android_jar" -d "$out/classes" \
  $(find "$root/tools/tests/am/stubs" -name '*.java' -print) \
  "$root/probes/compile-stubs/android/content/res/CompatibilityInfo.java" \
  "$root/runtime/framework/am/ApplicationProcessRegistry.java" \
  "$root/runtime/framework/am/SystemServiceBindings.java" \
  "$root/runtime/framework/am/ActiveServices.java" \
  "$root/runtime/framework/am/ServiceRecord.java" \
  "$root/runtime/framework/am/IntentBindRecord.java" \
  "$root/runtime/framework/am/ConnectionRecord.java" \
  "$root/runtime/framework/pm/PackageRecords.java" \
  "$root/runtime/framework/pm/InstalledPackageRecord.java" \
  "$root/runtime/framework/pm/InstalledManifestMetadata.java" \
  "$root/runtime/framework/pm/InstalledResourceValue.java" \
  "$root/runtime/framework/pm/InstalledApplicationInfo.java" \
  "$root/runtime/framework/pm/InstalledServiceInfo.java" \
  "$root/tools/tests/am/ActiveServicesProcessGoneTest.java"
"$java_home/bin/java" -ea -cp "$out/classes:$android_jar" \
  dev.darwinart.runtime.am.ActiveServicesProcessGoneTest
