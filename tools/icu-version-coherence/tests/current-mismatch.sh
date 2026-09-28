#!/bin/bash
set -euo pipefail
export LC_ALL=C

script_dir="$(cd "$(dirname "$0")" && pwd)"
gate="$script_dir/../check.sh"
output="$(mktemp "${TMPDIR:-/tmp}/icu-coherence-test.XXXXXX")"
trap 'rm -f "$output"' EXIT

set +e
"$gate" >"$output" 2>&1
status=$?
set -e

[[ "$status" == 2 ]] || {
  cat "$output" >&2
  echo "expected current ICU mismatch exit 2, got $status" >&2
  exit 1
}
grep -Fx 'java.icu_base_name=android/icu/impl/data/icudt68b' "$output" >/dev/null
grep -Fx 'icu4c.library.version=76.1' "$output" >/dev/null
grep -Fx 'icu4c.data.version=76.1' "$output" >/dev/null
grep -Fx 'resource.icuver.DataVersion=present' "$output" >/dev/null
grep -Fx 'resource.root_bundle=present' "$output" >/dev/null
grep -Fx 'resource.converter.UTF-8=present' "$output" >/dev/null
grep -Fx 'resource.collation.root=present' "$output" >/dev/null
grep -Fx 'resource.break_iterator.root=present' "$output" >/dev/null
grep -F 'icu-coherence.result=mismatch:java(68)!=library(76),java(68)!=data(76)' \
  "$output" >/dev/null

echo 'icu-version-coherence current-mismatch: PASS'
