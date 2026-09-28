#!/usr/bin/env bash
# Build and run P0 experiment 4. Binaries go outside the worktree.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
out="${P0_OUT:-${TMPDIR:-/tmp}/aim-p0}/04-linux-signals"
mkdir -p "$out"
c="$here/../common"
clang -O2 -Wall -I"$c" -o "$out/signals" "$here/signals.c" "$here/guest_sig.S" \
  "$c/lx_host.c" "$c/lx_trampoline.S" "$c/lx_guest.S"
"$out/signals"
