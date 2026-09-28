#!/usr/bin/env bash
# Build and run P0 experiment 2. Binaries go outside the worktree.
# Usage: run.sh [repeat-count]  (extra runs only report pass/fail; races)
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
out="${P0_OUT:-${TMPDIR:-/tmp}/aim-p0}/02-clone-threads"
mkdir -p "$out"
c="$here/../common"
clang -O2 -Wall -I"$c" -o "$out/clone" "$here/clone.c" "$here/guest_clone.S" "$c/lx_host.c" "$c/lx_trampoline.S" "$c/lx_guest.S"
"$out/clone"
fails=0
for ((i = 1; i < ${1:-1}; i++)); do
  "$out/clone" >/dev/null 2>&1 || fails=$((fails + 1))
done
if [[ ${1:-1} -gt 1 ]]; then echo "repeat: $fails of $((${1} - 1)) extra runs failed"; fi
