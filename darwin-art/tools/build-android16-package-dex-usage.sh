#!/bin/bash
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
# Verifies originals and builds host-only API fixtures separately. Their method
# bodies are never copied into the runtime output; javac uses signatures only.
bash "$root/tools/test-android16-package-dex-usage.sh"
output="$root/_build/package-dex-usage-runtime"
mkdir -p "$output/classes"
# Original service definitions come only from the pinned services.jar at runtime.
# The separate test output provides compile signatures, never packaged bodies.
javac --release 8 -sourcepath '' -cp "$root/_build/package-dex-usage-tests/classes" \
  -d "$output/classes" \
  "$root/runtime/framework/pm/DexInstructionSets.java" "$root/runtime/framework/pm/DexUsageStore.java"
# Keep the real original storage call in the wrapper, not a local replacement.
javap -classpath "$output/classes" -c -p com.android.server.pm.dex.DexUsageStore | \
  rg 'PackageDexUsage.record:' > /dev/null
# Explicit allowlist, not a recursive jar of a directory containing compile APIs.
jar cf "$output/package-dex-usage.jar" \
  -C "$output/classes" com/android/server/pm/dex/DexUsageStore.class \
  -C "$output/classes" dev/darwinart/runtime/pm/DexInstructionSets.class
java -ea -cp "$output/package-dex-usage.jar:$root/_build/package-dex-usage-tests/classes" \
  com.android.server.pm.dex.PackageDexUsageTest
echo 'Runtime usage adapters: PASS (2 wrapper classes; original definitions supplied by services.jar)'
