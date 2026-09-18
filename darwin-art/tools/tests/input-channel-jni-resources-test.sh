#!/bin/sh
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
stage=$(mktemp -d "${TMPDIR:-/tmp}/input-channel-jni.XXXXXX")
trap 'rm -rf -- "$stage"' EXIT
clang++ -std=c++20 -O1 -g -Wall -Wextra -Werror \
  -fsanitize=address,undefined -fno-omit-frame-pointer \
  -I"$root" -I"$root/compat" -I"$root/_aosp/libnativehelper/include_jni" \
  "$root/tools/tests/input-channel-jni-resources-test.cc" -o "$stage/test"
"$stage/test"
echo 'InputChannel JNI resources: unwind/transfer/pending-exception PASS'
