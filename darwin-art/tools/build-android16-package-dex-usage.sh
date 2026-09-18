#!/bin/bash
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
# Production adapter compilation does not execute or read host test fixtures.
# Original-source behavioral verification is the independently runnable
# test-android16-package-dex-usage.sh, not a producer of product compile inputs.
output="$root/_build/package-dex-usage-runtime"
mkdir -p "$output"
generation="$(mktemp -d "$output/generation.XXXXXX")"
mkdir -p "$generation/classes" "$generation/signatures"
sdk_root="${ANDROID_SDK_ROOT:-${ANDROID_HOME:-$HOME/Library/Android/sdk}}"
platform="${DARWIN_ART_SUPPORT_PLATFORM:-$sdk_root/platforms/android-36/android.jar}"
[[ -f "$platform" ]]
"${DARWIN_ART_SUPPORT_JAVAC:-javac}" --release 8 -sourcepath '' -cp "$platform" \
  -d "$generation/signatures" \
  "$root/runtime/framework/compile-stubs/dalvik/system/VMRuntime.java" \
  "$root/runtime/framework/compile-stubs/com/android/server/pm/dex/PackageDexUsage.java"
# Separate signature outputs are compile dependencies, never program definitions.
"${DARWIN_ART_SUPPORT_JAVAC:-javac}" --release 8 -sourcepath '' -cp "$generation/signatures:$platform" \
  -d "$generation/classes" \
  "$root/runtime/framework/pm/DexInstructionSets.java" "$root/runtime/framework/pm/DexUsageStore.java"
# Keep the real original storage call in the wrapper, not a local replacement.
"${DARWIN_ART_SUPPORT_JAVAP:-javap}" -classpath "$generation/classes" -c -p com.android.server.pm.dex.DexUsageStore | \
  rg 'PackageDexUsage.record:' > /dev/null
# Explicit allowlist, not a recursive jar of a directory containing compile APIs.
"${DARWIN_ART_SUPPORT_JAR:-jar}" cf "$generation/package-dex-usage.jar" \
  -C "$generation/classes" com/android/server/pm/dex/DexUsageStore.class \
  -C "$generation/classes" dev/darwinart/runtime/pm/DexInstructionSets.class
entries="$("${DARWIN_ART_SUPPORT_JAR:-jar}" tf "$generation/package-dex-usage.jar" | rg '\.class$' | sort)"
expected=$'com/android/server/pm/dex/DexUsageStore.class\ndev/darwinart/runtime/pm/DexInstructionSets.class'
[[ "$entries" == "$expected" ]]
# Publish a complete single file atomically; readers never see a partial JAR.
mv "$generation/package-dex-usage.jar" "$output/package-dex-usage.jar"
echo 'Runtime usage adapters: PASS (2 wrapper classes; original definitions supplied by services.jar)'
