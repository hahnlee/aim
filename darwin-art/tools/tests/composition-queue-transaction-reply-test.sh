#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
stage="$(mktemp -d "${TMPDIR:-/tmp}/composition-queue-transaction-reply.XXXXXX")"
trap 'rm -rf -- "$stage"' EXIT

sdk="$(xcrun --sdk macosx --show-sdk-path)"
xcrun clang++ -arch arm64 -isysroot "$sdk" -std=c++20 -O1 -g \
  -Wall -Wextra -Werror -pthread -fsanitize=address,undefined \
  -fno-omit-frame-pointer -I"$root" \
  "$root/compat/surfaceflinger/composition_queue.cc" \
  "$root/compat/surfaceflinger/transaction_reply.cc" \
  "$root/tools/tests/composition-queue-transaction-reply-test.cc" \
  -o "$stage/test"

ASAN_OPTIONS=detect_leaks=0:halt_on_error=1 \
UBSAN_OPTIONS=halt_on_error=1 \
  "$stage/test"
