#!/bin/sh
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../../.." && pwd)
stage=$(mktemp -d "${TMPDIR:-/tmp}/installed-activity-info.XXXXXX")
trap 'rm -rf -- "$stage"' EXIT HUP INT TERM
javac --release 8 -encoding UTF-8 -d "$stage" \
  "$root"/tools/tests/pm/android/content/pm/*.java \
  "$root/tools/tests/pm/activity-seam/dev/darwinart/runtime/pm/InstalledApplicationInfo.java" \
  "$root/runtime/framework/pm/InstalledPackageRecord.java" \
  "$root/runtime/framework/pm/InstalledActivityInfo.java" \
  "$root/tools/tests/pm/InstalledActivityInfoTest.java"
java -cp "$stage" InstalledActivityInfoTest
