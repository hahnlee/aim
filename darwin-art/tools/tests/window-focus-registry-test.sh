#!/bin/sh
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
stage=$(mktemp -d "${TMPDIR:-/tmp}/window-focus-registry.XXXXXX")
trap 'rm -rf -- "$stage"' EXIT HUP INT TERM
javac --release 8 -encoding UTF-8 -d "$stage" \
  "$root/runtime/framework/wm/WindowFocusRegistry.java" \
  "$root/tools/tests/wm/WindowFocusRegistryTest.java"
java -cp "$stage" dev.darwinart.runtime.wm.WindowFocusRegistryTest
