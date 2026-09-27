#!/usr/bin/env bash
# Build and run P1 experiment 1. Binaries go outside the worktree.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
out="${P1_OUT:-${TMPDIR:-/tmp}/darwin-art-p1}/01-binder-transport"
mkdir -p "$out"
clang -O2 -Wall -o "$out/transport" "$here/transport.c"
for mode in direct relayed socket; do "$out/transport" "$mode"; done
