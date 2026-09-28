#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
stage="$(mktemp -d "${TMPDIR:-/tmp}/egl-context-dispatch.XXXXXX")"
trap 'rm -rf -- "$stage"' EXIT
cxx="${CXX:-$(xcrun --find clang++)}"
sdk="$(xcrun --sdk macosx --show-sdk-path)"

"$cxx" -arch arm64 -isysroot "$sdk" -std=c++20 -O1 -g \
  -Wall -Wextra -Werror -pthread -I"$root" \
  "$root/compat/graphics/egl_context_dispatch.cc" \
  "$root/tools/tests/egl-context-dispatch-test.cc" \
  -o "$stage/test"

"$stage/test" 0 >"$stage/off.log" 2>&1
"$stage/test" 1 >"$stage/on.log" 2>&1
"$stage/test" 2 >"$stage/rejected.log" 2>&1
if [[ -s "$stage/off.log" ]]; then
  echo "egl context dispatch: debug-off unexpectedly emitted diagnostics" >&2
  cat "$stage/off.log" >&2
  exit 1
fi
rg -q '^ART Android EGL: eglCreateContext pid=[0-9]+ native_tid=[0-9]+ display=0x101 config=0x202 share=0x303 result=0xc0de attributes=\[0x3038\]$' "$stage/on.log"
rg -q '^ART Android EGL: eglMakeCurrent pid=[0-9]+ native_tid=[0-9]+ display=0x101 draw=0x404 read=0x505 context=0x303 result=1$' "$stage/on.log"
[[ "$(wc -l <"$stage/on.log")" -eq 2 ]]
rg -q '^ART Android EGL: eglCreateContext .* result=0 attributes=\[0x3038\]$' "$stage/rejected.log"
[[ "$(wc -l <"$stage/rejected.log")" -eq 1 ]]
echo "actual EGL dispatch: backend rejection forwarded once; fixture thread-local error retained PASS"
echo "actual EGL context dispatch module: exact args/results, null PFN rejection, threaded call, diagnostics on/off PASS"
