#!/bin/sh
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
stage=$(mktemp -d /tmp/darwin-art-client-receipt.XXXXXX)
trap 'rm -rf -- "$stage"' EXIT
sdk=$(xcrun --sdk macosx --show-sdk-path)
xcrun clang -std=c11 -Wall -Wextra -Werror -x c -fsyntax-only \
  -include "$root/compat/surfaceflinger/commit_receipt.h" /dev/null
xcrun clang++ -arch arm64 -isysroot "$sdk" -std=c++20 -O1 -g \
  -Wall -Wextra -Werror -fsanitize=address,undefined -I"$root" \
  "$root/compat/surfaceflinger/socket_transport.cc" \
  "$root/compat/surfaceflinger/client_receipt.cc" \
  "$root/tools/tests/surfaceflinger-client-receipt-test.cc" \
  -o "$stage/test"
ASAN_OPTIONS=detect_leaks=0:halt_on_error=1 UBSAN_OPTIONS=halt_on_error=1 \
  "$stage/test"
xcrun clang++ -arch arm64 -isysroot "$sdk" -std=c++20 -Wall -Wextra -Werror \
  -fsyntax-only "$root/compat/surfaceflinger/client_transport_darwin.mm"
