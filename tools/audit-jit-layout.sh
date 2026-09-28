#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
build="$root/_build/jit-layout-audit"
mkdir -p "$build"
clang++ -std=c++20 -O2 -Wall -Wextra -Werror \
  "$root/tools/jit-layout-smoke.cc" -o "$build/jit-layout-smoke"
codesign --force --sign - --options runtime \
  --entitlements "$root/config/aim-host.entitlements" "$build/jit-layout-smoke"
"$build/jit-layout-smoke"
"$build/jit-layout-smoke" --fragmented
