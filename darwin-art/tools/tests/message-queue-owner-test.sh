#!/bin/sh
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
stage=$(mktemp -d "${TMPDIR:-/tmp}/message-queue-owner.XXXXXX")
trap 'rm -rf -- "$stage"' EXIT
clang++ -std=c++20 -O1 -g -fsanitize=address,undefined \
  -fno-omit-frame-pointer -Wall -Wextra -Werror \
  -Wno-nullability-completeness -I"$root" -I"$root/compat" \
  -idirafter "$root/_aosp/libnativehelper-full/include_jni" \
  "$root/runtime/framework/looper/message_queue_jni.cc" \
  "$root/tools/tests/message-queue-owner-test.cc" -o "$stage/test"
"$stage/test"
echo 'MessageQueue exact owner and pending-exception lookup: PASS'
