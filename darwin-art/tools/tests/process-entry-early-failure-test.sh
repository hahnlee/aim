#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
if [[ $# -ne 2 ]]; then
  echo "usage: $0 GRAPHICS_PRODUCT_DYLIB HEADLESS_PRODUCT_DYLIB" >&2
  exit 2
fi
graphics_library="$1"
headless_library="$2"
for library in "$graphics_library" "$headless_library"; do
  [[ "$library" = /* && -f "$library" ]] || {
    echo "process-entry early failure: product dylib is not an absolute file: $library" >&2
    exit 2
  }
done

stage="$(mktemp -d "${TMPDIR:-/tmp}/process-entry-early-failure.XXXXXX")"
trap 'rm -rf -- "$stage"' EXIT
sdk="$(xcrun --sdk macosx --show-sdk-path)"
cxx="$(xcrun --find clang++)"
flags=(
  -arch arm64
  -isysroot "$sdk"
  -std=c++20
  -O1
  -g
  -Wall
  -Wextra
  -Werror
  -I"$root/include"
  -I"$root/tools/bionic-process-state-facade/include"
)

# This is an ABI-only executable. It does not recompile or relink production
# sources; each invocation resolves the real entry/process-state exports from
# the supplied product dylib in a fresh process.
"$cxx" "${flags[@]}" \
  "$root/tools/tests/process-entry-early-failure-test.cc" \
  -o "$stage/process-entry-early-failure-test"

for flavor in "$graphics_library" "$headless_library"; do
  for test_case in invalid-host-services invalid-graphics-context; do
    "$stage/process-entry-early-failure-test" "$flavor" "$test_case"
  done
done

echo "process-entry early failure: both product flavors, host-services/graphics validation, and pre-VM lifecycle closure PASS"
