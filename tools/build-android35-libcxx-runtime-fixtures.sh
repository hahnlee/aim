#!/bin/bash
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
base="$root/tools/android35-libcxx-acceptance"
exception="$root/tools/android35-libcxx-exception-acceptance"
build="$root/_build/android35-libcxx-runtime-fixtures"

lock_value() {
  local file="$1"
  local key="$2"
  awk -F= -v key="$key" '$1 == key {print $2}' "$file"
}

fail() {
  echo "android35-libcxx-runtime-fixtures: $*" >&2
  exit 1
}

ndk_revision="$(lock_value "$base/sources.lock" NDK_REVISION)"
sdk_root="${ANDROID_SDK_ROOT:-${ANDROID_HOME:-/Users/${USER:?USER is required}/Library/Android/sdk}}"
ndk_root="${ANDROID_NDK_ROOT:-${ANDROID_NDK_HOME:-$sdk_root/ndk/$ndk_revision}}"
[[ "$(basename "$ndk_root")" == "$ndk_revision" ]] ||
  fail "Android NDK is not pinned r28c: $ndk_root"

toolchain="$ndk_root/toolchains/llvm/prebuilt/darwin-x86_64"
clang="$toolchain/bin/aarch64-linux-android35-clang++"
libcxx="$toolchain/sysroot/usr/lib/aarch64-linux-android/libc++_shared.so"
libc="$toolchain/sysroot/usr/lib/aarch64-linux-android/35/libc.so"
libdl="$toolchain/sysroot/usr/lib/aarch64-linux-android/35/libdl.so"
libunwind="$toolchain/lib/clang/19/lib/linux/aarch64/libunwind.a"
for input in "$clang" "$libcxx" "$libc" "$libdl" "$libunwind"; do
  [[ -e "$input" ]] || fail "missing pinned input: $input"
done

stage="$(mktemp -d "${TMPDIR:-/tmp}/aim-libcxx-runtime.XXXXXX")"
trap 'rm -rf "$stage"' EXIT
mkdir -p "$stage/collections" "$stage/exception"

"$clang" -shared -fPIC -O2 -fvisibility=hidden -fno-exceptions -fno-rtti \
  -nostdlib -nodefaultlibs -Wl,-soname,libaim_libcxx_consumer.so \
  -Wl,-z,now,-z,relro,--hash-style=sysv -Wl,--build-id=none \
  "$base/consumer.cc" -Wl,--no-as-needed "$libcxx" "$libc" -Wl,--as-needed \
  -o "$stage/collections/libaim_libcxx_consumer.so"

"$clang" -shared -fPIC -O2 -fvisibility=hidden -nostdlib -nodefaultlibs \
  -Wl,-soname,libaim_libcxx_exception.so \
  -Wl,-z,now,-z,relro,--hash-style=sysv -Wl,--build-id=none \
  "$exception/exception_consumer.cc" "$libunwind" \
  -Wl,--no-as-needed "$libcxx" "$libc" "$libdl" -Wl,--as-needed \
  -o "$stage/exception/libaim_libcxx_exception.so"

hash_file() {
  shasum -a 256 "$1" | awk '{print $1}'
}

[[ "$(hash_file "$libcxx")" == "$(lock_value "$base/sources.lock" LIBCXX_SHA256)" ]] ||
  fail "libc++ hash drift"
[[ "$(hash_file "$stage/collections/libaim_libcxx_consumer.so")" == \
   "$(lock_value "$base/sources.lock" CONSUMER_ELF_SHA256)" ]] ||
  fail "collections fixture hash drift"
[[ "$(hash_file "$stage/exception/libaim_libcxx_exception.so")" == \
   "$(lock_value "$exception/sources.lock" STATIC_UNWIND_ELF_SHA256)" ]] ||
  fail "exception fixture hash drift"

# Graph discovery rejects symlinks by design, so each isolated sibling graph
# receives its own regular-file copy of the pinned runtime.
cp "$libcxx" "$stage/collections/libc++_shared.so"
cp "$libcxx" "$stage/exception/libc++_shared.so"
mkdir -p "$build"
for graph in collections exception; do
  mkdir -p "$build/$graph"
  cp "$stage/$graph/"*.so "$build/$graph/"
done

echo "android35-libcxx-runtime-fixtures: PASS graphs=collections+exception sibling-libcxx=real isolated=2 self-test=JNI_OnLoad"
