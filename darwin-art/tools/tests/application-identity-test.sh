#!/bin/zsh
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
sdk="$(xcrun --sdk macosx --show-sdk-path)"
tmp="$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-application-identity.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT

"$(xcrun --find clang++)" -arch arm64 -isysroot "$sdk" -std=c++20 \
  -ObjC++ -fobjc-arc -O1 -g -Wall -Wextra -Werror \
  -I"$root" -I"$root/compat" \
  "$root/compat/window/application_identity.mm" \
  "$root/tools/tests/application-identity-test.mm" \
  -framework AppKit -o "$tmp/application-identity-test"

"$tmp/application-identity-test"
