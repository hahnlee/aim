#!/bin/sh
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
stage=$(mktemp -d /tmp/darwin-art-client-transport.XXXXXX)
trap 'rm -rf -- "$stage"' EXIT
sdk=$(xcrun --sdk macosx --show-sdk-path)
xcrun clang++ -arch arm64 -isysroot "$sdk" -std=c++20 -O1 -g -pthread \
  -Wall -Wextra -Werror -fsanitize=address,undefined -I"$root" \
  "$root/compat/surfaceflinger/client_transport_darwin.mm" \
  "$root/compat/surfaceflinger/client_receipt.cc" \
  "$root/compat/surfaceflinger/socket_transport.cc" \
  "$root/compat/surfaceflinger/transaction_reply.cc" \
  "$root/compat/surfaceflinger/retained_layer_state.cc" \
  "$root/tools/tests/surfaceflinger-client-transport-test.cc" \
  -framework IOSurface -framework CoreFoundation -o "$stage/test"
ASAN_OPTIONS=detect_leaks=0:halt_on_error=1 UBSAN_OPTIONS=halt_on_error=1 \
  "$stage/test" "$stage/endpoint.sock"
