#!/bin/zsh
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
sdk="$(xcrun --sdk macosx --show-sdk-path)"
tmp="$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-owned-timed-task.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT

common=(
  -arch arm64 -isysroot "$sdk" -std=c++20 -Wall -Wextra -Werror -pthread
  -I"$root" -I"$root/compat"
  -I"$root/tools/bionic-socket-broker-adapter/include"
  -I"$root/tools/bionic-errno-tls/include"
  -I"$root/_aosp/frameworks/native/include"
)

"$(xcrun --find clang++)" "${common[@]}" \
  "$root/compat/looper/android_looper_owner.cc" \
  "$root/compat/looper/reusable_task.cc" \
  "$root/tools/tests/android-looper-owned-timed-task-test.cc" \
  -o "$tmp/android-looper-owned-timed-task-test"

"$tmp/android-looper-owned-timed-task-test"
