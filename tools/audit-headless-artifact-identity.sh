#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
artifact="$root/_build/runtime-link-probe/libaim_runtime.dylib"
graph="$root/_build/native-graph/build.ninja"
ninja="$root/_aosp/external/skia/third_party/ninja/ninja"
log_dir="$(mktemp -d "${TMPDIR:-/tmp}/aim-headless-identity.XXXXXX")"
trap 'rm -rf -- "$log_dir"' EXIT

run_audit() {
  local log="$1"
  (cd "$root" && cargo run -q -p aim-xtask -- native-graph --out "$graph" &&
    "$ninja" -f "$graph" headless-runtime-audit) >"$log" 2>&1
}

# Force the output edge once so the first sample proves Ninja owns and can
# materialize the artifact; the second invocation then verifies a warm no-op.
(cd "$root" && "$ninja" -f "$graph" -t clean "$artifact") >"$log_dir/clean.log" 2>&1 || {
  echo "headless-artifact-identity: failed to clean prior graph output" >&2
  cat "$log_dir/clean.log" >&2
  exit 1
}
run_audit "$log_dir/first.log"
[[ -f "$artifact" ]] || {
  echo "headless-artifact-identity: missing artifact after first audit: $artifact" >&2
  exit 1
}
first_sha="$(shasum -a 256 "$artifact" | awk '{print $1}')"

run_audit "$log_dir/second.log"
second_sha="$(shasum -a 256 "$artifact" | awk '{print $1}')"
[[ "$first_sha" == "$second_sha" ]] || {
  echo "headless-artifact-identity: hash changed between identical audits" >&2
  echo "first=$first_sha second=$second_sha" >&2
  exit 1
}

grep -a -F 'ninja: no work to do.' "$log_dir/second.log" >/dev/null || {
  echo "headless-artifact-identity: second graph invocation was not a warm no-op" >&2
  exit 1
}

grep -a -F 'audit-runtime-link: C ABI dylib closure complete undefined=0' "$log_dir/first.log" >/dev/null || {
  echo "headless-artifact-identity: runtime-link audit did not complete" >&2
  exit 1
}
printf 'headless-artifact-identity: PASS sha256=%s artifact=%s\n' "$second_sha" "$artifact"
