#!/bin/bash
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
out="$root/_build/binder-rpc-identity-test"
mkdir -p "$out"
xcrun clang++ -std=c++23 -arch arm64 -Wall -Wextra -Werror \
  -I"$root/compat" \
  "$root/tools/tests/binder-rpc-identity-test.cc" \
  "$root/compat/binder/calling_identity.cc" \
  "$root/compat/binder/peer_credentials.cc" \
  "$root/compat/binder/rpc_identity.cc" \
  -o "$out/binder-rpc-identity-test"
"$out/binder-rpc-identity-test"
