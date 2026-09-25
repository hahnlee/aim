#!/bin/sh
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../../.." && pwd)
stage=$(mktemp -d "${TMPDIR:-/tmp}/installed-launch-intents.XXXXXX")
trap 'rm -rf -- "$stage"' EXIT HUP INT TERM
javac --release 8 -encoding UTF-8 -d "$stage" \
  "$root"/tools/tests/pm/android/content/pm/*.java \
  "$root"/tools/tests/pm/intent-stubs/android/content/*.java \
  "$root"/tools/tests/pm/intent-stubs/android/content/pm/*.java \
  "$root"/tools/tests/pm/intent-stubs/android/net/*.java \
  "$root/tools/tests/pm/activity-seam/dev/darwinart/runtime/pm/InstalledApplicationInfo.java" \
  "$root/runtime/framework/pm/InstalledPackageRecord.java" \
  "$root/runtime/framework/pm/InstalledActivityInfo.java" \
  "$root/runtime/framework/pm/PackageRecords.java" \
  "$root/runtime/framework/pm/InstalledLaunchIntents.java" \
  "$root/tools/tests/pm/InstalledLaunchIntentsTest.java"
java -cp "$stage" dev.darwinart.runtime.pm.InstalledLaunchIntentsTest
