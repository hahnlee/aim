#!/bin/bash
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
test_dir="$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-application-registration.XXXXXX")"
trap 'rm -f "$test_dir/test"; rmdir "$test_dir"' EXIT
xcrun clang++ -std=c++20 -Wall -Wextra -Werror \
  -fsanitize=address,undefined -fno-omit-frame-pointer \
  -I"$root" -I"$root/_aosp/libnativehelper-full/include_jni" \
  "$root/tools/tests/application-registration.cc" \
  "$root/runtime/framework/app/process_registration.cc" \
  -o "$test_dir/test"
"$test_dir/test"
