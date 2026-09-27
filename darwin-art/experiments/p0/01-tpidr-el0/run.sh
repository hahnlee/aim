#!/usr/bin/env bash
# Build and run P0 experiment 1. Binaries go outside the worktree.
set -uo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
out="${P0_OUT:-${TMPDIR:-/tmp}/darwin-art-p0}/01-tpidr-el0"
mkdir -p "$out"
clang -O2 -Wall -o "$out/tpidr" "$here/tpidr.c" || exit 1
clang -O2 -Wall -o "$out/tls_rewrite" "$here/tls_rewrite.c" || exit 1
"$out/tpidr" "${1:-10}" "${2:-64}"
echo "tpidr exit status: $? (1 = a user-written TPIDR_EL0 was clobbered)"
"$out/tls_rewrite"
