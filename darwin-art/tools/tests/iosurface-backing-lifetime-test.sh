#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
stage="$(mktemp -d "${TMPDIR:-/tmp}/iosurface-backing-lifetime.XXXXXX")"
trap 'rm -rf -- "$stage"' EXIT
sdk="$(xcrun --sdk macosx --show-sdk-path)"
cxx="$(xcrun --find clang++)"
"$cxx" -arch arm64 -isysroot "$sdk" -std=c++20 -ObjC++ -O1 -g -Wall -Wextra -Werror -pthread -I"$root" "$root/compat/surfaceflinger/iosurface_backing.mm" "$root/compat/surfaceflinger/composition_queue.cc" "$root/tools/tests/iosurface-backing-lifetime-test.mm" -framework CoreFoundation -framework IOSurface -o "$stage/iosurface-backing-lifetime-test"
"$stage/iosurface-backing-lifetime-test"
