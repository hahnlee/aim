#!/bin/sh
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
stage=$(mktemp -d "${TMPDIR:-/tmp}/receiver-jni-retirement.XXXXXX")
trap 'rm -rf -- "$stage"' EXIT
clang++ -std=c++20 -O1 -g -Wall -Wextra -Werror -pthread \
  -fsanitize=address,undefined -fno-omit-frame-pointer \
  -I"$root" -I"$root/compat" -I"$root/_aosp/libnativehelper/include_jni" \
  "$root/runtime/framework/input/receiver_jni_resources.cc" \
  "$root/runtime/framework/input/receiver_admission.cc" \
  "$root/runtime/framework/input/receiver_registry.cc" \
  "$root/tools/tests/receiver-jni-retirement-test.cc" -o "$stage/test"
"$stage/test"
echo 'Receiver JNI retirement: cleanup-before-notify/exact-once/resource-only handle PASS'
