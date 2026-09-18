#!/bin/bash
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
stage=$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-root-client-jni.XXXXXX")
trap 'rm -rf "$stage"' EXIT
jdk=/opt/homebrew/opt/openjdk@17/libexec/openjdk.jdk/Contents/Home
"$jdk/bin/javac" -d "$stage" \
  "$root/tools/tests/desktop-root-client-jni/android/view/InputChannel.java" \
  "$root/tools/tests/desktop-root-client-jni/DesktopRootClient.java" \
  "$root/runtime/framework/wm/DesktopForegroundAuthority.java" \
  "$root/tools/tests/desktop-root-client-jni/DesktopForegroundAuthorityTest.java"
clang++ -std=c++20 -fobjc-arc -fblocks -Wall -Wextra -Werror -O1 -g \
  -I"$root" -I"$jdk/include" -I"$jdk/include/darwin" \
  "$root/runtime/framework/wm/desktop_root_client_jni.mm" \
  "$root/runtime/framework/wm/desktop_foreground_authority_jni.cc" \
  "$root/compat/window/desktop_foreground_provider.cc" \
  "$root/compat/window/desktop_root_target.mm" \
  "$root/compat/window/desktop_root_events.mm" \
  "$root/tools/tests/desktop-root-client-jni/test.mm" \
  -L"$jdk/lib/server" -ljvm -Wl,-rpath,"$jdk/lib/server" \
  -framework AppKit -framework CoreFoundation -framework ApplicationServices -o "$stage/test"
"$stage/test" "$stage" observe
"$stage/test" "$stage" cancel
"$stage/test" "$stage" deferred
"$stage/test" "$stage" pending
"$stage/test" "$stage" attach
"$stage/test" "$stage" reject
