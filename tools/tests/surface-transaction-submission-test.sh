#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
ndk="${ANDROID_NDK_HOME:-$HOME/Library/Android/sdk/ndk/28.2.13676358}"
ndk_include="$ndk/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include"
[[ -f "$ndk_include/android/surface_control.h" ]] || {
  echo "surface-transaction-submission: pinned NDK include is missing: $ndk_include" >&2
  exit 2
}

stage="$(mktemp -d "${TMPDIR:-/tmp}/surface-transaction-submission.XXXXXX")"
trap 'rm -rf -- "$stage"' EXIT
sdk="$(xcrun --sdk macosx --show-sdk-path)"
cxx="$(xcrun --find clang++)"
sanitizers="${SANITIZER:-address,undefined}"
flags=(
  -arch arm64
  -isysroot "$sdk"
  -std=c++20
  -O1
  -g
  -Wall
  -Wextra
  -Werror
  -Wno-nullability-completeness
  -pthread
  "-fsanitize=$sanitizers"
  -fno-omit-frame-pointer
  -I"$root"
  -idirafter "$ndk_include"
)

# Only the submission and already-separated transaction lifetime owners are
# linked.  Port readiness, opaque handles, fences and callback behavior are
# explicit test doubles; no product or probe object is included.
"$cxx" "${flags[@]}" \
  "$root/compat/window/surface_transaction_lifetime.cc" \
  "$root/compat/window/surface_transaction_submission.cc" \
  "$root/tools/tests/surface-transaction-submission-test.cc" \
  -o "$stage/surface-transaction-submission-test"
ASAN_OPTIONS=detect_leaks=0:halt_on_error=1 \
UBSAN_OPTIONS=halt_on_error=1 \
  "$stage/surface-transaction-submission-test"

