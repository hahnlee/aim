#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
stage="$(mktemp -d "${TMPDIR:-/tmp}/output-registry.XXXXXX")"
trap 'rm -rf -- "$stage"' EXIT
sdk="$(xcrun --sdk macosx --show-sdk-path)"
cxx="$(xcrun --find clang++)"
"$cxx" -arch arm64 -isysroot "$sdk" -std=c++20 -ObjC++ -O1 -g -Wall -Wextra -Werror -pthread -I"$root" "$root/compat/surfaceflinger/iosurface_backing.mm" "$root/compat/surfaceflinger/output_registry.cc" "$root/tools/tests/output-registry-test.mm" -framework CoreFoundation -framework IOSurface -o "$stage/output-registry-test"
"$stage/output-registry-test"
