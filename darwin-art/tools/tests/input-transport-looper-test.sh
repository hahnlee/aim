#!/bin/zsh
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
cxx="$(xcrun --find clang++)"
sdk="$(xcrun --sdk macosx --show-sdk-path)"
tmp="$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-input-looper.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT
"$cxx" -arch arm64 -isysroot "$sdk" -std=c++20 -Wall -Wextra -Werror -pthread \
  -fsanitize=address,undefined -fno-omit-frame-pointer \
  -I"$root" -I"$root/compat" \
  -I"$root/tools/bionic-socket-broker-adapter/include" \
  -I"$root/tools/bionic-errno-tls/include" \
  -I"$root/_aosp/frameworks/native/include" \
  "$root/compat/looper/android_looper_owner.cc" \
  "$root/compat/looper/reusable_task.cc" \
  "$root/runtime/framework/input/transport_registration_authority.cc" \
  "$root/runtime/framework/input/input_transport.cc" \
  "$root/runtime/framework/input/input_framed_reader.cc" \
  "$root/runtime/framework/input/input_resource_progress.cc" \
  "$root/runtime/framework/input/input_transport_pump.cc" \
  "$root/runtime/framework/input/input_transport_readiness.cc" \
  "$root/tools/tests/input-transport-looper-test.cc" \
  -o "$tmp/input-transport-looper-test"
"$tmp/input-transport-looper-test"
