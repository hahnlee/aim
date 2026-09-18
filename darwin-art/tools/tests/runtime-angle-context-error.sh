#!/bin/bash
set -euo pipefail

# The production wrapper is compiled directly into this isolated test. Its
# two provider dylibs are scoped test backends (not ANGLE and not production
# providers), used solely to return deterministic BAD_MATCH/BAD_ATTRIBUTE.
root="$(cd "$(dirname "$0")/../.." && pwd)"
stage="$(mktemp -d "${TMPDIR:-/tmp}/runtime-angle-context-error.XXXXXX")"
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
  -I"$root/_aosp/frameworks/native/opengl/include"
  -I"$root/_aosp/frameworks/base/libs/androidfw/include"
)

clang -arch arm64 -dynamiclib -fPIC \
  "$root/tools/tests/runtime-angle-context-error-backend.c" \
  -o "$stage/libEGL.dylib"
clang -arch arm64 -dynamiclib -fPIC \
  "$root/tools/tests/runtime-angle-context-error-gles-backend.c" \
  -o "$stage/libGLESv2.dylib"

clang++ -std=c++20 -arch arm64 -O0 -ffunction-sections -fdata-sections \
  -Wall -Wextra -Wno-unused-function -include jni.h "${includes[@]}" \
  "$root/tools/tests/runtime-angle-context-error-test.cc" \
  "$root/compat/graphics/egl_context_dispatch.cc" \
  "$root/compat/graphics/egl_error_state.cc" \
  -Wl,-dead_strip -Wl,-undefined,dynamic_lookup \
  -o "$stage/test"

for debug in off on; do
  for error in 0x3009 0x3004; do
    if [[ "$debug" == on ]]; then
      DARWIN_ART_DEBUG_GRAPHICS_DSO=1 \
        DARWIN_ART_ANGLE_DIRECTORY="$stage" \
        DARWIN_ART_TEST_EGL_ERROR="$error" "$stage/test" "$error"
    else
      DARWIN_ART_ANGLE_DIRECTORY="$stage" \
        DARWIN_ART_TEST_EGL_ERROR="$error" "$stage/test" "$error"
    fi
  done
done
echo "mock ANGLE backend: BAD_MATCH/BAD_ATTRIBUTE one-shot guest eglGetError, debug on/off PASS"
