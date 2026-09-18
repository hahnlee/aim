#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
ndk="${ANDROID_NDK_HOME:-$HOME/Library/Android/sdk/ndk/28.2.13676358}"
ndk_include="$ndk/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include"
[[ -f "$ndk_include/android/hardware_buffer.h" ]] || exit 2
stage="$(mktemp -d "${TMPDIR:-/tmp}/metal-shared-event-provider.XXXXXX")"
trap 'rm -rf -- "$stage"' EXIT
sdk="$(xcrun --sdk macosx --show-sdk-path)"
cxx="$(xcrun --find clang++)"
"$cxx" -arch arm64 -isysroot "$sdk" -std=c++20 -O1 -g \
  -DDARWIN_ART_METAL_SHARED_EVENT_TESTING \
  -Wall -Wextra -Werror -Wno-nullability-completeness -pthread \
  -fsanitize=address,undefined -fno-omit-frame-pointer \
  -I"$root" -I"$root/tools/bionic-socket-broker-adapter/include" \
  -idirafter "$ndk_include" \
  "$root/compat/graphics/metal_shared_event_provider.mm" \
  "$root/compat/surfaceflinger/metal_composer.mm" \
  "$root/tools/tests/metal-shared-event-provider-test.mm" \
  -framework Foundation -framework IOSurface -framework Metal \
  -o "$stage/metal-shared-event-provider"
ASAN_OPTIONS=detect_leaks=0:halt_on_error=1 \
UBSAN_OPTIONS=halt_on_error=1 "$stage/metal-shared-event-provider"
