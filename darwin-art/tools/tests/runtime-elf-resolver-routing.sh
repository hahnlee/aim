#!/bin/bash
set -euo pipefail

# Independently compiles the production resolver TU twice. The provider and
# namespace definitions below are test-only mocks; no runtime/archive/client
# producer is reused, and this does not modify the active acceptance gate.
root="$(cd "$(dirname "$0")/../.." && pwd)"
stage="$(mktemp -d "${TMPDIR:-/tmp}/runtime-elf-resolver-routing.XXXXXX")"
trap 'rm -rf -- "$stage"' EXIT

includes=(
  -I"$root/compat"
  -I"$root/compat/loader"
  -I"$root/compat/network"
  -I"$root/include"
  -I"$root/crates/darwin-art-elf-loader/include"
  -I"$root/tools/bionic-dso-lifecycle-facade/include"
  -I"$root/tools/bionic-provider-namespace/include"
  -I"$root/tools/android-jni-proxy/include"
  -I"$root/_aosp/libnativehelper/include_jni"
  -I"$root/_aosp/libnativehelper/include"
  -I"$root/_aosp/system/core/base/include"
  -I"$root/_aosp/frameworks/native/libs/nativewindow/include"
  -I"$root/_aosp/frameworks/native/include"
  -I"$root/_aosp/frameworks/native/libs/arect/include"
  -I"$root/_aosp/frameworks/base/libs/androidfw/include"
)
common=(-std=c++20 -arch arm64 -Wall -Wextra -Werror -Wno-unused-function -include jni.h "${includes[@]}")

clang++ "${common[@]}" -c "$root/compat/darwin_runtime_elf_resolver.cc" \
  -o "$stage/resolver-headless.o"
clang++ "${common[@]}" -DDARWIN_ART_REAL_GRAPHICS \
  -c "$root/compat/darwin_runtime_elf_resolver.cc" -o "$stage/resolver-graphics.o"
# Link the real 39-entry graphics inventory. The test supplies only the one
# callable AndroidBitmap function it invokes; unused AOSP entries are allowed
# to remain dynamic lookup symbols in this isolated executable.
clang++ "${common[@]}" -DDARWIN_ART_REAL_GRAPHICS \
  -c "$root/compat/loader/graphics_ndk_symbols.cc" -o "$stage/graphics-table.o"
clang++ "${common[@]}" -c \
  "$root/tools/tests/runtime-elf-resolver-graphics-mocks.cc" \
  -o "$stage/graphics-mocks.o"

clang++ "${common[@]}" "$root/tools/tests/runtime-elf-resolver-routing-test.cc" \
  "$stage/resolver-headless.o" -Wl,-undefined,dynamic_lookup -o "$stage/headless"
clang++ "${common[@]}" -DDARWIN_ART_REAL_GRAPHICS \
  "$root/tools/tests/runtime-elf-resolver-routing-test.cc" \
  "$stage/resolver-graphics.o" "$stage/graphics-table.o" "$stage/graphics-mocks.o" \
  -Wl,-undefined,dynamic_lookup -o "$stage/graphics"

"$stage/headless"
"$stage/graphics"
echo "actual resolver TU: headless rejection + graphics libjnigraphics route PASS"
