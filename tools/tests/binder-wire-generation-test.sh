#!/bin/sh
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
stage=$(mktemp -d "${TMPDIR:-/tmp}/binder-wire-generation.XXXXXX")
trap 'rm -rf "$stage"' EXIT HUP INT TERM
clang++ -std=c++20 -O1 -g -Wall -Wextra -Werror -pthread \
  -fsanitize=address,undefined -fno-omit-frame-pointer \
  -Wl,-dead_strip -I"$root" -I"$root/compat" \
  -I"$root/_aosp/libnativehelper/include_jni" \
  "$root/tools/tests/binder-wire-generation-test.cc" \
  "$root/compat/binder/wire_channel_lifetime.cc" \
  "$root/compat/binder/calling_identity.cc" \
  "$root/compat/binder/peer_credentials.cc" \
  "$root/compat/binder/remote_binder_jni.cc" \
  "$root/compat/binder/remote_binder_identity_jni.cc" \
  "$root/runtime/framework/wm/root_key_server_jni.cc" \
  -o "$stage/test"
"$stage/test"
