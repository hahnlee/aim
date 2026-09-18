#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
stage="$(mktemp -d "${TMPDIR:-/tmp}/iosurface-output-owner.XXXXXX")"
trap 'rm -rf -- "$stage"' EXIT
sdk="$(xcrun --sdk macosx --show-sdk-path)"
cxx="$(xcrun --find clang++)"
"$cxx" -arch arm64 -isysroot "$sdk" -std=c++20 -O1 -g -Wall -Wextra -Werror -pthread -I"$root" "$root/compat/surfaceflinger/output_owner.cc" "$root/tools/tests/iosurface-output-owner-lifetime-test.mm" -o "$stage/iosurface-output-owner-lifetime-test"
"$stage/iosurface-output-owner-lifetime-test"
