#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/../../.." && pwd)"
java_home="${JAVA_HOME:-/opt/homebrew/opt/openjdk@17}"
android_jar="${ANDROID_PLATFORM_JAR:-$HOME/Library/Android/sdk/platforms/android-36/android.jar}"
out="$(mktemp -d "${TMPDIR:-/tmp}/bound-service-process-launcher-test.XXXXXX")"
trap 'rm -rf -- "$out"' EXIT
mkdir -p "$out/classes"
"$java_home/bin/javac" --release 8 -encoding UTF-8 -cp "$android_jar" -d "$out/classes" \
  $(find "$root/tools/tests/am/stubs" -name '*.java' -print) \
  "$root/probes/compile-stubs/android/content/res/CompatibilityInfo.java" \
  "$root/runtime/framework/am/ApplicationProcessRegistry.java" \
  "$root/runtime/framework/am/SystemServiceBindings.java" \
  "$root/runtime/framework/am/ActiveServices.java" \
  "$root/runtime/framework/am/ServiceRecord.java" \
  "$root/runtime/framework/am/StartedServiceRequests.java" \
  "$root/runtime/framework/am/IntentBindRecord.java" \
  "$root/runtime/framework/am/ConnectionRecord.java" \
  "$root/runtime/framework/am/ServiceConnectionOwner.java" \
  "$root/runtime/framework/am/ServiceConnectionIndex.java" \
  "$root/runtime/framework/am/ServiceConnectionDeathRegistration.java" \
  "$root/runtime/framework/am/ServiceNotificationController.java" \
  "$root/runtime/framework/am/ServiceProcessLaunchController.java" \
  "$root/runtime/framework/am/ServiceConnectionResourceController.java" \
  "$root/runtime/framework/am/ServiceLifecycleOperation.java" \
  "$root/runtime/framework/am/ServiceLifecycleController.java" \
  "$root/runtime/framework/am/BoundServiceProcessLauncher.java" \
  "$root/runtime/framework/am/ProcessLaunchTransport.java" \
  "$root/runtime/framework/am/PackageQueries.java" \
  "$root/tools/tests/am/TestServices.java" \
  "$root/tools/tests/am/BoundServiceProcessLauncherTest.java"
"$java_home/bin/java" -ea -cp "$out/classes:$android_jar" \
  dev.darwinart.runtime.am.BoundServiceProcessLauncherTest
