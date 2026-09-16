#!/bin/bash
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
stage="$(mktemp -d "${TMPDIR:-/tmp}/android-jni-class-lookup.XXXXXX")"
trap 'rm -rf "$stage"' EXIT

# Link the actual production callback. The test supplies only the current-env
# boundary and fixture observation variable, not a replacement lookup body.
xcrun clang++ -std=c++20 -O1 -g -Wall -Wextra -Werror \
  -fsanitize=address,undefined -fno-omit-frame-pointer \
  -ffunction-sections -fdata-sections \
  -I"$root/compat" -I"$root/include" \
  -I"$root/_build/nativehelper-foundation/source/libnativehelper/include_jni" \
  -I"$root/_build/android-elf-jni-fixture/generated" \
  -I"$root/crates/darwin-art-elf-loader/include" \
  -I"$root/tools/android-jni-proxy/include" \
  -I"$root/tools/bionic-dso-lifecycle-facade/include" \
  -I"$root/tools/bionic-provider-namespace/include" \
  "$root/compat/darwin_jni_proxy_lookup.cc" \
  "$root/compat/jni/android_varargs.cc" \
  "$root/compat/jni/method_call.cc" \
  "$root/tools/android-jni-class-lookup-test.cc" \
  -Wl,-dead_strip -o "$stage/test"
"$stage/test"
