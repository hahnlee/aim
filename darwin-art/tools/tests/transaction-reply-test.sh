#!/bin/bash
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
stage="$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-transaction-reply.XXXXXX")"
trap 'rm -rf -- "$stage"' EXIT
xcrun clang++ -std=c++20 -O1 -g -Wall -Wextra -Werror \
  -fsanitize=address,undefined -fno-omit-frame-pointer \
  "$root/compat/surfaceflinger/transaction_reply.cc" \
  "$root/compat/surfaceflinger/service_ingress.cc" \
  "$root/tools/tests/transaction-reply-test.cc" -o "$stage/test"
"$stage/test"
