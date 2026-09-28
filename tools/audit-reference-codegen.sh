#!/usr/bin/env bash
set -euo pipefail
reference_root="$(cd "$(dirname "$0")/.." && pwd)"
reference_build="$reference_root/_build/reference-codegen-audit"
mkdir -p "$reference_build"
clang++ -std=c++20 -O2 -DVIXL_INCLUDE_TARGET_A64 \
  -I"$reference_root/compat" \
  -I"$reference_root/_build/foundation/patched-source/libartbase" \
  -I"$reference_root/_aosp/art/libartbase" \
  -I"$reference_root/_aosp/system/libbase/include" \
  -I"$reference_root/_aosp/external/fmtlib/include" \
  -I"$reference_root/_aosp/external/vixl/src" \
  "$reference_root/tools/reference-codegen-smoke.cc" \
  "$reference_root/compat/darwin_jit_memory.cc" \
  "$reference_root/_build/jit-compiler/libart-compiler-darwin.a" \
  -o "$reference_build/reference-codegen-smoke"
codesign --force --sign - --options runtime \
  --entitlements "$reference_root/config/aim-host.entitlements" \
  "$reference_build/reference-codegen-smoke"
"$reference_build/reference-codegen-smoke"
