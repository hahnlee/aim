#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
build="$root/_build/jit-memory-audit"
mkdir -p "$build"
ulimit -c 0
clang++ -std=c++20 -O2 -Wall -Wextra -Werror -I"$root/compat" \
  "$root/tools/jit-memory-smoke.cc" "$root/compat/darwin_jit_memory.cc" \
  -o "$build/jit-memory-smoke"
codesign --force --sign - --options runtime \
  --entitlements "$root/config/aim-host.entitlements" "$build/jit-memory-smoke"
"$build/jit-memory-smoke"
for mode in protected-write execute-while-writing; do
  set +e
  "$build/jit-memory-smoke" "--$mode" >"$build/$mode.log" 2>&1
  status=$?
  set -e
  if [[ "$status" != 138 && "$status" != 139 ]]; then
    echo "JIT memory: expected protection fault for $mode, got $status" >&2
    exit 1
  fi
  grep -F "JIT memory negative phase=$mode armed" "$build/$mode.log" >/dev/null || {
    echo "JIT memory: protection fault for $mode lacked armed-phase evidence" >&2
    exit 1
  }
done
echo 'JIT memory: PASS signed MAP_JIT nested scopes concurrent execution positive+negative W^X gates'
