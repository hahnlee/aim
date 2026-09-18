#!/bin/zsh
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
cxx="$(xcrun --find clang++)"
sdk="$(xcrun --sdk macosx --show-sdk-path)"
tmp="$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-reusable-looper.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT

common=(
  -arch arm64 -isysroot "$sdk" -std=c++20 -Wall -Wextra -Werror -pthread
  -fsanitize=address,undefined -fno-omit-frame-pointer
  -I"$root" -I"$root/compat"
  -I"$root/tools/bionic-socket-broker-adapter/include"
  -I"$root/tools/bionic-errno-tls/include"
  -I"$root/_aosp/frameworks/native/include"
)

"$cxx" "${common[@]}" \
  "$root/compat/looper/android_looper_owner.cc" \
  "$root/compat/looper/reusable_task.cc" \
  "$root/tools/tests/reusable-looper-task-test.cc" \
  -o "$tmp/reusable-looper-task-test"

"$tmp/reusable-looper-task-test"
