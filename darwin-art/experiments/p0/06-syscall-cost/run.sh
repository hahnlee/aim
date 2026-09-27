#!/usr/bin/env bash
# Build and run P0 experiment 6. Binaries go outside the worktree.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
out="${P0_OUT:-${TMPDIR:-/tmp}/darwin-art-p0}/06-syscall-cost"
mkdir -p "$out"
c="$here/../common"
clang -O2 -Wall -I"$c" -mgeneral-regs-only -c -o "$out/lean.o" "$here/lean.c"
clang -O2 -Wall -I"$c" -o "$out/cost" "$here/cost.c" "$here/cost.S" "$out/lean.o" \
  "$c/lx_host.c" "$c/lx_trampoline.S"
"$out/cost"
