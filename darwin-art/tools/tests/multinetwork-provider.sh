#!/bin/bash
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
output="$(mktemp "${TMPDIR:-/tmp}/darwin-art-multinetwork.XXXXXX")"
trap 'rm -f "$output"' EXIT

xcrun clang++ -std=c++20 -Wall -Wextra -Werror \
  -I"$root/compat" \
  -I"$root/tools/bionic-errno-tls/include" \
  -I"$root/tools/bionic-socket-broker-adapter/include" \
  "$root/compat/network/multinetwork.cc" \
  "$root/tools/tests/multinetwork-provider.cc" \
  -o "$output"
"$output"
echo "multinetwork-provider: PASS version=LIBANDROID unbound=0 unknown=ENONET socket=broker"
