#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/../../.." && pwd)"
java_home="${JAVA_HOME:-/opt/homebrew/opt/openjdk@17}"
out="$(mktemp -d "${TMPDIR:-/tmp}/notification-records.XXXXXX")"
trap 'rm -rf -- "$out"' EXIT

"$java_home/bin/javac" --release 8 -encoding UTF-8 -d "$out" \
  $(find "$root/tools/tests/notification/stubs" "$root/tools/tests/notification/src" \
      -name '*.java' -print) \
  "$root/runtime/framework/notification/NotificationRecords.java"
"$java_home/bin/java" -ea -cp "$out" dev.darwinart.runtime.notification.NotificationRecordsTest
