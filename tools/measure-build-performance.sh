#!/usr/bin/env bash
set -euo pipefail

# Measure the existing build gates without changing their inputs or outputs.
# Each command gets an isolated temporary log.  The command's own stdout and
# stderr are kept out of the repository; only the compact summary is printed.

usage() {
  cat <<'USAGE'
usage: tools/measure-build-performance.sh [options]

Measure one or more existing build commands:
  --cargo-check       cargo check --workspace -v (default; -v exposes Fresh)
  --graphics-bootstrap
                      cargo run -p art-bootstrap -- build-runtime-graphics-bootstrap
  --audit-graphics    cargo run -p art-bootstrap -- audit-runtime-graphics-link
  --all               run all three commands
  --keep-output       retain temporary logs and print their directory
  -h, --help          show this help

The measured commands are not modified.  A non-zero command is reported and
causes this script to exit non-zero after all selected commands have run.
USAGE
}

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$script_dir/.." && pwd)"
tmp_root="$(mktemp -d "${TMPDIR:-/tmp}/aim-build-perf.XXXXXX")"
keep_output=0
run_cargo_check=0
run_graphics_bootstrap=0
run_audit_graphics=0
explicit_selection=0
overall_status=0

cleanup() {
  if [[ "$keep_output" -eq 1 ]]; then
    printf 'build-performance: logs=%s\n' "$tmp_root"
  else
    rm -rf "$tmp_root"
  fi
}
trap cleanup EXIT

for arg in "$@"; do
  case "$arg" in
    --cargo-check)
      run_cargo_check=1
      explicit_selection=1
      ;;
    --graphics-bootstrap)
      run_graphics_bootstrap=1
      explicit_selection=1
      ;;
    --audit-graphics)
      run_audit_graphics=1
      explicit_selection=1
      ;;
    --all)
      run_cargo_check=1
      run_graphics_bootstrap=1
      run_audit_graphics=1
      explicit_selection=1
      ;;
    --keep-output)
      keep_output=1
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "build-performance: unknown option: $arg" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if [[ "$explicit_selection" -eq 0 ]]; then
  run_cargo_check=1
fi

summary="$tmp_root/summary.tsv"
printf 'name\tstatus\twall_seconds\tcompiled_lines\tcached_lines\tlog\n' >"$summary"

count_matches() {
  local pattern="$1"
  local file="$2"
  # rg returns 1 for no matches; that is data, not a script failure.
  (rg -i -c "$pattern" "$file" || true) | awk -F: '{ total += $NF } END { print total + 0 }'
}

run_case() {
  local name="$1"
  shift
  local log="$tmp_root/$name.log"
  local timing="$tmp_root/$name.time"
  local status
  local wall
  local compiled
  local cached

  printf 'build-performance: running %s\n' "$name"
  # BSD and GNU time both support -o. Keep the command's complete output in
  # the log so compile/cache signals are visible, while timing stays parseable.
  if (cd "$root" && /usr/bin/time -p -o "$timing" "$@" >"$log" 2>&1); then
    status=0
  else
    status=$?
    overall_status=1
  fi

  wall="$(awk '$1 == "real" { print $2; exit }' "$timing")"
  [[ -n "$wall" ]] || wall="unknown"
  # These are intentionally heuristic counters. They make Cargo's `Compiling`
  # versus `Fresh`, and the existing shell gate's archive/cache messages
  # visible without instrumenting or changing either build command.
  compiled="$(count_matches '(^|[[:space:]])(Compiling|Checking|compile|compiled|building|built|archive)([[:space:]]|:|$)' "$log")"
  cached="$(count_matches '(Fresh|cached|cache hit|reused|up[- ]to[- ]date|skipping)' "$log")"
  printf '%s\t%s\t%s\t%s\t%s\t%s\n' \
    "$name" "$status" "$wall" "$compiled" "$cached" "$log" >>"$summary"
}

if [[ "$run_cargo_check" -eq 1 ]]; then
  # -v only exposes Cargo's Fresh/Compiling lines; it does not change the
  # dependency graph or produced artifacts.
  run_case cargo-check cargo check --workspace -v
fi
if [[ "$run_graphics_bootstrap" -eq 1 ]]; then
  run_case graphics-bootstrap cargo run -v -p art-bootstrap -- build-runtime-graphics-bootstrap
fi
if [[ "$run_audit_graphics" -eq 1 ]]; then
  run_case audit-graphics cargo run -v -p art-bootstrap -- audit-runtime-graphics-link
fi

printf '\n%-22s %-6s %-12s %-10s %-9s\n' \
  'command' 'status' 'wall_seconds' 'compiled' 'cached'
printf '%-22s %-6s %-12s %-10s %-9s\n' \
  '----------------------' '------' '------------' '----------' '---------'
tail -n +2 "$summary" | while IFS=$'\t' read -r name status wall compiled cached _log; do
  printf '%-22s %-6s %-12s %-10s %-9s\n' \
    "$name" "$status" "$wall" "$compiled" "$cached"
done

if [[ "$overall_status" -ne 0 ]]; then
  echo "build-performance: one or more selected commands failed" >&2
fi
exit "$overall_status"
