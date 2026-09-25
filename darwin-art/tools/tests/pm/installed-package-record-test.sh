#!/bin/sh
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../../.." && pwd)
stage=$(mktemp -d "${TMPDIR:-/tmp}/installed-package-record.XXXXXX")
trap 'rm -rf -- "$stage"' EXIT HUP INT TERM
javac --release 8 -encoding UTF-8 -d "$stage" \
  "$root/runtime/framework/pm/InstalledPackageRecord.java" \
  "$root/tools/tests/pm/InstalledPackageRecordTest.java"
DARWIN_ART_ANDROID_PACKAGE_ROOT=/profile/packages \
  java -ea -cp "$stage" dev.darwinart.runtime.pm.InstalledPackageRecordTest
