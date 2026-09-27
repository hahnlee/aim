#!/usr/bin/env bash
# Build and run P0 experiment 3. Binaries go outside the worktree.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
out="${P0_OUT:-${TMPDIR:-/tmp}/darwin-art-p0}/03-futex"
mkdir -p "$out"
clang -O2 -Wall -o "$out/futex" "$here/futex.c"
clang -O2 -Wall -o "$out/timer_slack" "$here/timer_slack.c"
"$out/futex"
"$out/timer_slack"
