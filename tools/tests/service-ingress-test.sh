#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
stage="$(mktemp -d "${TMPDIR:-/tmp}/service-ingress.XXXXXX")"
trap 'rm -rf -- "$stage"' EXIT
cxx="$(xcrun --find clang++)"
sdk="$(xcrun --sdk macosx --show-sdk-path)"
"$cxx" -arch arm64 -isysroot "$sdk" -std=c++20 -O1 -g -Wall -Wextra -Werror -pthread -I"$root" "$root/compat/surfaceflinger/service_ingress.cc" "$root/tools/tests/service-ingress-test.cc" -o "$stage/service-ingress-test"
"$stage/service-ingress-test"
