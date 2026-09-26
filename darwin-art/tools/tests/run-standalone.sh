#!/usr/bin/env bash
# Runs every standalone test script under tools/tests (component, contract and
# JVM tests that need no running profile, window or APK). Scripts that need
# inputs this runner cannot supply are listed with the reason instead of being
# silently dropped. Usage: tools/tests/run-standalone.sh [PATTERN]
set -uo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
pattern="${1:-}"
timeout_seconds="${DARWIN_ART_TEST_TIMEOUT:-600}"
logs="$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-standalone-tests.XXXXXX")"

# script -> reason it is not run here.
skip_reason() {
  case "$1" in
    tools/tests/run-standalone.sh) echo "this runner" ;;
    tools/tests/process-entry-early-failure-test.sh) echo "needs graphics and headless product dylibs" ;;
    tools/tests/surface-backing-product-test.sh) echo "needs a product dylib and mode" ;;
    tools/tests/system-compat-artifact.sh | tools/tests/system-compat-partitions.sh | \
      tools/tests/system-services.sh) echo "needs the pinned system image" ;;
    tools/tests/android-system-image-build.sh) echo "builds the system image" ;;
    tools/tests/binder-recipient/*) echo "needs its fixture arguments" ;;
    *) return 1 ;;
  esac
}

passed=0
failed=()
skipped=0
cd "$root" || exit 1
# Audits that check pinned tables against their generators, outside tools/tests.
audits=(tools/bionic-provider-namespace/audit.sh)
for script in tools/tests/*.sh tools/tests/*/*.sh "${audits[@]}"; do
  [[ -f "$script" ]] || continue
  [[ -z "$pattern" || "$script" == *"$pattern"* ]] || continue
  if reason="$(skip_reason "$script")"; then
    printf 'SKIP %s (%s)\n' "$script" "$reason"
    skipped=$((skipped + 1))
    continue
  fi
  log="$logs/$(echo "$script" | tr '/' '_').log"
  if perl -e 'alarm shift; exec @ARGV' "$timeout_seconds" bash "$script" >"$log" 2>&1; then
    passed=$((passed + 1))
  else
    failed+=("$script")
    printf 'FAIL %s (log %s)\n' "$script" "$log"
  fi
done

printf 'standalone tests: passed=%d failed=%d skipped=%d logs=%s\n' \
  "$passed" "${#failed[@]}" "$skipped" "$logs"
[[ "${#failed[@]}" -eq 0 ]]
