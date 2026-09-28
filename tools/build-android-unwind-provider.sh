#!/bin/bash
set -euo pipefail
export LC_ALL=C

root="$(cd "$(dirname "$0")/.." && pwd)"
source "$root/tools/android35-libcxx-exception-acceptance/sources.lock"
sdk_root="${ANDROID_SDK_ROOT:-${ANDROID_HOME:-$HOME/Library/Android/sdk}}"
ndk_root="${ANDROID_NDK_ROOT:-${ANDROID_NDK_HOME:-$sdk_root/ndk/$NDK_REVISION}}"
toolchain="$ndk_root/toolchains/llvm/prebuilt/darwin-x86_64"
clang="$toolchain/bin/aarch64-linux-android35-clang++"
objcopy="$toolchain/bin/llvm-objcopy"
nm="$toolchain/bin/llvm-nm"
readelf="$toolchain/bin/llvm-readelf"
archive="$toolchain/lib/clang/19/lib/linux/aarch64/libunwind.a"
libc="$toolchain/sysroot/usr/lib/aarch64-linux-android/35/libc.so"
libdl="$toolchain/sysroot/usr/lib/aarch64-linux-android/35/libdl.so"
output="${1:-$root/_build/android-unwind-provider/libaim_android_unwind.so}"

fail() { echo "android-unwind-provider: $*" >&2; exit 1; }
for input in "$clang" "$objcopy" "$nm" "$readelf" "$archive" "$libc" "$libdl"; do
  [[ -e "$input" ]] || fail "missing pinned input: $input"
done
[[ "$(shasum -a 256 "$archive" | awk '{print $1}')" == \
   "$LIBUNWIND_ARCHIVE_SHA256" ]] || fail "libunwind archive hash drift"

stage="$(mktemp -d "${TMPDIR:-/tmp}/aim-unwind-provider.XXXXXX")"
trap 'rm -rf "$stage"' EXIT
renamed="$stage/libunwind-renamed.a"
cp "$archive" "$renamed"

symbols=(
  _Unwind_Backtrace _Unwind_DeleteException _Unwind_FindEnclosingFunction
  _Unwind_Find_FDE _Unwind_ForcedUnwind _Unwind_GetCFA
  _Unwind_GetDataRelBase _Unwind_GetGR _Unwind_GetIP _Unwind_GetIPInfo
  _Unwind_GetLanguageSpecificData _Unwind_GetRegionStart _Unwind_GetTextRelBase
  _Unwind_RaiseException _Unwind_Resume _Unwind_Resume_or_Rethrow
  _Unwind_SetGR _Unwind_SetIP
)
rename_args=()
for symbol in "${symbols[@]}"; do
  rename_args+=(--redefine-sym "$symbol=aim_internal$symbol")
done
"$objcopy" "${rename_args[@]}" "$renamed"

"$clang" -c -fPIC "$root/tools/android-unwind-provider/exports.S" \
  -o "$stage/exports.o"
mkdir -p "$(dirname "$output")"
"$clang" -shared -fPIC -nostdlib -nodefaultlibs \
  -Wl,-z,now,-z,relro,--hash-style=sysv -Wl,--build-id=none \
  -Wl,-soname,libaim_android_unwind.so \
  -Wl,--version-script,"$root/tools/android-unwind-provider/exports.map" \
  "$stage/exports.o" -Wl,--whole-archive "$renamed" -Wl,--no-whole-archive \
  -Wl,--no-as-needed "$libc" "$libdl" -Wl,--as-needed -o "$output"

for symbol in "${symbols[@]}"; do
  "$nm" -D --defined-only "$output" | grep -F " T $symbol@@LIBC_R" >/dev/null ||
    fail "missing versioned export: $symbol"
done
! "$nm" -D --defined-only "$output" | grep -F 'aim_internal' >/dev/null ||
  fail "internal libunwind symbol escaped"
"$readelf" -d "$output" | grep -F 'Shared library: [libc.so]' >/dev/null ||
  fail "libc dependency missing"
"$readelf" -d "$output" | grep -F 'Shared library: [libdl.so]' >/dev/null ||
  fail "libdl dependency missing"
echo "android-unwind-provider: PASS exports=${#symbols[@]} output=$output"
