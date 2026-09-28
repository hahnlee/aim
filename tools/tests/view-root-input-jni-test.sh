#!/bin/sh
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
source="$root/runtime/framework/input/view_root_input_jni.cc"
stage=$(mktemp -d "${TMPDIR:-/tmp}/view-root-input-jni.XXXXXX")
trap 'rm -rf -- "$stage"' EXIT

# The ViewRoot owner must remain independent of channel-state layout and must
# leave JNI's pending exception untouched.  Keep this as an isolated source
# contract so it can run without a runtime/product link.
if rg -n 'ExceptionClear|DarwinInputChannelState|->channel|SetFrameworkViewRootFocus|FocusFrameworkViewRoot' "$source"; then
  echo 'ViewRoot JNI owner crossed the channel-state/exception boundary' >&2
  exit 1
fi

sdk_path=$(xcrun --sdk macosx --show-sdk-path)
clang++ -arch arm64 -isysroot "$sdk_path" -std=c++20 \
  -Wall -Wextra -Werror -Wno-nullability-completeness -pthread \
  -I"$root" -I"$root/compat" \
  -idirafter "$root/_aosp/libnativehelper-full/include_jni" \
  -fsyntax-only "$source"

clang++ -arch arm64 -isysroot "$sdk_path" -std=c++20 -O1 -g \
  -fsanitize=address,undefined -fno-omit-frame-pointer \
  -Wall -Wextra -Werror -Wno-nullability-completeness -pthread \
  -I"$root" -I"$root/compat" \
  -idirafter "$root/_aosp/libnativehelper-full/include_jni" \
  "$source" "$root/runtime/framework/input/receiver_admission.cc" \
  "$root/runtime/framework/input/receiver_jni_resources.cc" \
  "$root/runtime/framework/input/receiver_focus_jni.cc" \
  "$root/runtime/framework/input/key_fallback.cc" \
  "$root/tools/tests/view-root-input-jni-test.cc" \
  -o "$stage/test"
"$stage/test"

echo 'ViewRoot JNI owner: opaque routing, reentrant-dispose, ACK, and syntax PASS'
