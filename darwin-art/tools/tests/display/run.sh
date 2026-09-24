#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/../../.." && pwd)"
out="$root/_build/display-configuration-test"

rm -rf "$out"
mkdir -p "$out"
javac --release 8 -encoding UTF-8 -d "$out" \
  "$root/tools/tests/display/stubs/android/graphics/Rect.java" \
  "$root/tools/tests/display/stubs/android/app/WindowConfiguration.java" \
  "$root/tools/tests/display/stubs/android/content/res/Configuration.java" \
  "$root/runtime/framework/display/BuiltInDisplayConfiguration.java" \
  "$root/runtime/framework/display/DisplayGeometry.java" \
  "$root/tools/tests/display/DisplayGeometryTest.java"
java -ea -cp "$out" dev.darwinart.runtime.display.DisplayGeometryTest
