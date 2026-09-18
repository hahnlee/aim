#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
ndk="${ANDROID_NDK_HOME:-$HOME/Library/Android/sdk/ndk/28.2.13676358}"
ndk_include="$ndk/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include"
[[ -f "$ndk_include/android/surface_control.h" ]] || {
  echo "native-window transaction admission: pinned NDK include is missing" >&2
  exit 2
}
stage="$(mktemp -d "${TMPDIR:-/tmp}/native-window-transaction-admission.XXXXXX")"
trap 'rm -rf -- "$stage"' EXIT
sdk="$(xcrun --sdk macosx --show-sdk-path)"
cxx="$(xcrun --find clang++)"
flags=(
  -arch arm64 -isysroot "$sdk" -std=c++20 -O1 -g -Wall -Wextra -Werror
  -Wno-nullability-completeness -pthread -fsanitize=address,undefined
  -fno-omit-frame-pointer -I"$root" -idirafter "$ndk_include"
)
"$cxx" "${flags[@]}" \
  -DASurfaceTransaction_create=TestLifetimeCreate \
  -DASurfaceTransaction_delete=TestLifetimeDelete \
  -Ddarwin_art_android_surface_transaction_set_buffer_callbacks_checked=TestLifetimeCallbacksChecked \
  -Ddarwin_art_android_surface_transaction_set_buffer_callbacks=TestLifetimeCallbacks \
  -c "$root/compat/window/surface_transaction_lifetime.cc" \
  -o "$stage/lifetime.o"
"$cxx" "${flags[@]}" \
  -Ddarwin_art_android_surface_transaction_set_buffer_with_cookie_checked=TestBuilderCookieAdapter \
  -c "$root/compat/window/surface_transaction_builder.cc" \
  -o "$stage/builder.o"
"$cxx" "${flags[@]}" \
  "$root/compat/window/native_window_transaction_consumer.cc" \
  "$root/compat/window/native_window_buffer_queue.cc" \
  "$root/compat/window/locked_surface.cc" \
  "$root/tools/tests/native-window-transaction-admission-test.cc" \
  "$stage/lifetime.o" \
  "$stage/builder.o" \
  -Wl,-undefined,dynamic_lookup -o "$stage/test"
ASAN_OPTIONS=detect_leaks=0:halt_on_error=1 \
UBSAN_OPTIONS=halt_on_error=1 \
  "$stage/test"
