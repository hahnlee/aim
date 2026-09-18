#!/bin/bash
set -euo pipefail
export LC_ALL=C

dir="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$dir/../.." && pwd)"
owner="$root/tools/bionic-socket-broker-adapter/src"
tmp="$(mktemp -d "${TMPDIR:-/tmp}/eventfd-owner.XXXXXX")"
trap 'find "$tmp" -depth -delete' EXIT

sdk="$(xcrun --sdk macosx --show-sdk-path)"
host_cxx="$(xcrun --find clang++)"
host_cc="$(xcrun --find clang)"
includes=(-I"$owner")
errno_includes=(-I"$root/tools/bionic-errno-tls/include"
                -I"$root/tools/bionic-errno-tls/generated")

for sanitizer in address,undefined; do
  output="$tmp/eventfd-owner-${sanitizer//,/-}"
  "$host_cc" -arch arm64 -isysroot "$sdk" -std=c17 -O1 -g \
    -Wall -Wextra -Werror "${errno_includes[@]}" \
    -fsanitize="$sanitizer" -fno-omit-frame-pointer \
    -c "$root/tools/bionic-errno-tls/src/errno_tls.c" \
    -o "$tmp/errno-${sanitizer//,/-}.o"
  "$host_cxx" -arch arm64 -isysroot "$sdk" -std=c++20 -O1 -g \
    -Wall -Wextra -Werror -Wpedantic -fsanitize="$sanitizer" \
    -fno-omit-frame-pointer "${includes[@]}" "${errno_includes[@]}" \
    "$dir/eventfd-owner-test.cc" "$owner/eventfd_owner.cc" \
    "$tmp/errno-${sanitizer//,/-}.o" -o "$output"
  ASAN_OPTIONS=detect_leaks=0:halt_on_error=1 \
    UBSAN_OPTIONS=halt_on_error=1 "$output"
done

echo 'eventfd-owner: PASS ASan+UBSan'
