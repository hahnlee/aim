#!/bin/bash
# Compile the production WindowRootFocusDecisions owner with test-only boundary
# stand-ins. This fixture is not a runtime/product or probe-linked acceptance.
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
source_root="$root/tools/tests/wm/window-root-decision"
[[ -f "$root/runtime/framework/wm/WindowRootFocusDecisions.java" ]] || exit 69
stage="$(mktemp -d "${TMPDIR:-/tmp}/window-root-decision-test.XXXXXX")"
cleanup() { rm -rf -- "$stage"; }
trap cleanup EXIT
mkdir -p "$stage/classes"
stubs=()
while IFS= read -r stub; do stubs+=("$stub"); done < <(
  find "$source_root/stubs" -type f -name '*.java' -print | sort
)
javac -d "$stage/classes" \
  "${stubs[@]}" \
  "$root/runtime/framework/wm/DesktopRootFocusDecision.java" \
  "$root/runtime/framework/wm/WindowRootFocusDecisions.java" \
  "$source_root/WindowRootFocusDecisionsTest.java"
java -cp "$stage/classes" dev.darwinart.runtime.wm.WindowRootFocusDecisionsTest
