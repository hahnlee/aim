#!/bin/bash
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
source "$root/upstream/android16-package-dex-usage.lock"
sources="$root/_aosp/android16-package-dex-usage"
output="$root/_build/package-dex-usage-tests"
mkdir -p "$sources" "$output/classes"
fetch() {
  local relative="$1" expected="$2" target="$sources/${1##*/}"
  if [[ -f "$target" ]] && [[ "$(shasum -a 256 "$target" | awk '{print $1}')" == "$expected" ]]; then return; fi
  local temporary
  temporary="$(mktemp "$sources/download.XXXXXX")"
  curl -fsSL "https://android.googlesource.com/platform/frameworks/base/+/$FRAMEWORKS_BASE_REVISION/services/core/java/com/android/server/pm/$relative?format=TEXT" | base64 -D > "$temporary"
  [[ "$(shasum -a 256 "$temporary" | awk '{print $1}')" == "$expected" ]]
  mv "$temporary" "$target"
}
fetch dex/PackageDexUsage.java "$PACKAGE_DEX_USAGE_SHA256"
fetch AbstractStatsBase.java "$ABSTRACT_STATS_BASE_SHA256"
# These platform doubles are TEST ONLY and never enter the runtime DEX.
test_sources=()
while IFS= read -r path; do test_sources+=("$path"); done < <(rg --files "$root/tools/tests/package-dex-usage" -g '*.java')
javac -d "$output/classes" "$sources/PackageDexUsage.java" "$sources/AbstractStatsBase.java" "${test_sources[@]}"
java -ea -cp "$output/classes" com.android.server.pm.dex.PackageDexUsageTest
