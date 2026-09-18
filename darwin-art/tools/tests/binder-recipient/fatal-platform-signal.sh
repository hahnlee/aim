#!/bin/bash
# Actual ART signal handler in an isolated fixture image, not a stub handler.
set -euo pipefail
[[ $# == 5 ]] || { echo 'expected five actual ART boot/support inputs' >&2; exit 64; }
root="$(cd "$(dirname "$0")/../../.." && pwd)"
out="$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-fatal-platform.XXXXXX")"
trap 'rm -rf "$out"' EXIT
env DARWIN_ART_TEST_FATAL_PLATFORM_SIGNAL=1 \
  bash "$root/tools/tests/binder-recipient/run.sh" "$@" >"$out/child.log" 2>&1 &
child=$!
for _ in {1..300}; do
  kill -0 "$child" 2>/dev/null || break
  sleep 0.1
done
if kill -0 "$child" 2>/dev/null; then
  kill -KILL "$child"
  wait "$child" || true
  tail -30 "$out/child.log"
  echo 'fatal platform child did not terminate within30s' >&2
  exit 1
fi
status=0
wait "$child" || status=$?
rg 'FATAL_TEST_REAL_ART_READY' "$out/child.log"
if rg 'FATAL_TEST_FINALIZER_RAN' "$out/child.log"; then
  echo 'fatal handler invoked atexit cleanup' >&2; exit 1
fi
[[ "$status" == 1 ]] || { tail -30 "$out/child.log"; echo "unexpected fatal status=$status" >&2; exit 1; }
echo 'actual ART Darwin fatal handler: nonzero bounded exit, no finalizer PASS'
