#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
stage="$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-process-exit.XXXXXX")"
trap 'rm -rf -- "$stage"' EXIT
clang -std=c17 -O2 -Wall -Wextra -Werror -Wpedantic \
  -I"$root/tools/bionic-process-state-facade/include" \
  "$root/tools/bionic-process-state-facade/src/process_exit.c" \
  "$root/tools/tests/bionic-process-exit-test.c" -o "$stage/test"
"$stage/test" 2>"$stage/stderr"
[[ "$(rg -c 'Bionic (_exit|exit) pid=' "$stage/stderr")" == 5 ]]
rg -q 'Bionic exit-frame pid=' "$stage/stderr"
# Exactly the two opt-in exec children emit frames; the default exit, _exit
# and signal-handler paths must not enter the unwinder.
[[ "$(rg 'Bionic exit-frame pid=' "$stage/stderr" | sed -E 's/.*pid=([0-9]+).*/\1/' | sort -u | wc -l | tr -d ' ')" == 2 ]]
awk '
  /Bionic (exit|_exit) pid=/ { child++ }
  /Bionic exit-frame pid=/ {
    if (child != 3 && child != 4) exit 1
    frames[child]++
  }
  END { if (child != 5 || !frames[3] || !frames[4]) exit 1 }
' "$stage/stderr"
echo 'Bionic process exit actual-TU: status, atexit/_exit, opt-in only, signal exit PASS'
