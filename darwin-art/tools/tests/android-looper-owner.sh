#!/bin/zsh
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
cxx="$(xcrun --find clang++)"
sdk="$(xcrun --sdk macosx --show-sdk-path)"
# The test links the actual extracted owners. Its test translation unit below
# supplies only a scoped broker transport mock, isolating the fd/wake boundary
# from the production central broker (no production provider is substituted).
tmp="$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-looper-owner.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT

common=(
  -arch arm64 -isysroot "$sdk" -std=c++20 -Wall -Wextra -Werror -pthread
  -I"$root" -I"$root/compat"
  -I"$root/tools/bionic-socket-broker-adapter/include"
  -I"$root/tools/bionic-errno-tls/include"
  -I"$root/_aosp/frameworks/native/include"
)

"$cxx" "${common[@]}" \
  "$root/compat/looper/android_looper_owner.cc" \
  "$root/compat/looper/reusable_task.cc" \
  "$root/compat/looper/android_choreographer_owner.cc" \
  "$root/tools/tests/android-looper-owner-test.cc" \
  -o "$tmp/android-looper-owner-test"

# The actual-owner TU covers exact owned removal, stale same-fd disposers,
# generation-safe callback return, and reentrant release outside the mutex.
output="$tmp/android-looper-owner-test.out"
"$tmp/android-looper-owner-test" | tee "$output"
rg -F 'FD generation/ownership' "$output" >/dev/null
