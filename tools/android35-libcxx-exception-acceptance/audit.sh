#!/bin/bash
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
here="$root/tools/android35-libcxx-exception-acceptance"
source "$here/sources.lock"

fail() {
  echo "android35-libcxx-exception-acceptance: $*" >&2
  exit 1
}

sdk_root="${ANDROID_SDK_ROOT:-${ANDROID_HOME:-/Users/${USER:?USER is required}/Library/Android/sdk}}"
ndk_root="${ANDROID_NDK_ROOT:-${ANDROID_NDK_HOME:-$sdk_root/ndk/$NDK_REVISION}}"
[[ "$(basename "$ndk_root")" == "$NDK_REVISION" ]] ||
  fail "Android NDK is not pinned r28c: $ndk_root"

toolchain="$ndk_root/toolchains/llvm/prebuilt/darwin-x86_64"
clang="$toolchain/bin/aarch64-linux-android35-clang++"
readelf="$toolchain/bin/llvm-readelf"
nm="$toolchain/bin/llvm-nm"
libcxx="$toolchain/sysroot/usr/lib/aarch64-linux-android/libc++_shared.so"
libc="$toolchain/sysroot/usr/lib/aarch64-linux-android/35/libc.so"
libdl="$toolchain/sysroot/usr/lib/aarch64-linux-android/35/libdl.so"
libunwind="$toolchain/lib/clang/19/lib/linux/aarch64/libunwind.a"
ownership="$root/tools/bionic-provider-namespace/generated/ownership.tsv"
for input in "$clang" "$readelf" "$nm" "$libcxx" "$libc" "$libdl" \
  "$libunwind" "$ownership"; do
  [[ -e "$input" ]] || fail "missing pinned input: $input"
done

hash_file() {
  shasum -a 256 "$1" | awk '{print $1}'
}

[[ "$(hash_file "$libcxx")" == "$LIBCXX_SHA256" ]] || fail "libc++ hash drift"
[[ "$(hash_file "$libunwind")" == "$LIBUNWIND_ARCHIVE_SHA256" ]] ||
  fail "libunwind archive hash drift"
[[ "$(hash_file "$here/exception_consumer.cc")" == "$CONSUMER_SOURCE_SHA256" ]] ||
  fail "exception consumer source hash drift"
[[ "$(hash_file "$here/exception.imports")" == "$STATIC_UNWIND_IMPORTS_SHA256" ]] ||
  fail "exception import lock drift"

stage="$(mktemp -d "${TMPDIR:-/tmp}/aim-libcxx-exception.XXXXXX")"
trap 'rm -rf "$stage"' EXIT
plain="$stage/libexception_plain.so"
accepted="$stage/libaim_libcxx_exception.so"
common=(
  -shared -fPIC -O2 -fvisibility=hidden -nostdlib -nodefaultlibs
  -Wl,-z,now,-z,relro,--hash-style=sysv -Wl,--build-id=none
)

# Baseline: prove why the normal NDK shared-library link cannot enter the
# current closed namespace.
"$clang" "${common[@]}" -Wl,-soname,libexception_plain.so \
  "$here/exception_consumer.cc" -Wl,--no-as-needed "$libcxx" "$libc" \
  -Wl,--as-needed -o "$plain"
[[ "$(hash_file "$plain")" == "$PLAIN_ELF_SHA256" ]] || fail "plain ELF hash drift"
[[ "$("$nm" -D --undefined-only "$plain" | grep -c ' _Unwind_Resume@LIBC_R$')" == 1 ]] ||
  fail "plain exception link no longer exposes the LIBC_R boundary"

# Accepted vertical slice: keep Android's unwinder and make the cleanup resume
# implementation local to this DSO. libc/libdl stubs version every remaining
# provider import so runtime routing remains exact.
"$clang" "${common[@]}" -Wl,-soname,libaim_libcxx_exception.so \
  "$here/exception_consumer.cc" "$libunwind" \
  -Wl,--no-as-needed "$libcxx" "$libc" "$libdl" -Wl,--as-needed \
  -o "$accepted"
[[ "$(hash_file "$accepted")" == "$STATIC_UNWIND_ELF_SHA256" ]] ||
  fail "static-unwind ELF hash drift"
[[ "$(stat -f '%z' "$accepted")" == "$EXPECTED_STATIC_UNWIND_SIZE" ]] ||
  fail "static-unwind ELF size drift"
file "$accepted" | grep -F 'ELF 64-bit LSB shared object, ARM aarch64' >/dev/null ||
  fail "accepted fixture is not Android AArch64 ELF"

"$nm" -D --undefined-only "$accepted" | awk '{print $2}' | LC_ALL=C sort \
  > "$stage/accepted.imports"
cmp -s "$here/exception.imports" "$stage/accepted.imports" ||
  fail "accepted fixture import manifest drift"
! grep -F '_Unwind_Resume' "$stage/accepted.imports" >/dev/null ||
  fail "accepted fixture still imports an external unwinder"

"$nm" -D --defined-only "$libcxx" | awk '{print $3}' | LC_ALL=C sort -u \
  > "$stage/libcxx.exports"
while IFS= read -r import; do
  if [[ "$import" == *@LIBC ]]; then
    symbol="${import%@LIBC}"
    soname=libc.so
    [[ "$symbol" == dl_iterate_phdr ]] && soname=libdl.so
    grep -F "$soname"$'\t'"$symbol"$'\tLIBC\t' "$ownership" >/dev/null ||
      fail "provider closure lacks $soname:$symbol@LIBC"
  else
    grep -Fx "$import" "$stage/libcxx.exports" >/dev/null ||
      fail "libc++ does not export consumer import: $import"
  fi
done < "$stage/accepted.imports"

"$readelf" -l -d -r -V --dyn-syms --wide "$accepted" > "$stage/accepted.readelf.txt"
[[ "$(grep -c '(NEEDED)' "$stage/accepted.readelf.txt")" == 3 ]] ||
  fail "accepted fixture DT_NEEDED count drift"
for needed in libc++_shared.so libc.so libdl.so; do
  grep -F "Shared library: [$needed]" "$stage/accepted.readelf.txt" >/dev/null ||
    fail "accepted fixture dependency drift: $needed"
done
grep -E '\(SONAME\).*\[libaim_libcxx_exception\.so\]' \
  "$stage/accepted.readelf.txt" >/dev/null || fail "accepted fixture SONAME drift"
grep -E '\(BIND_NOW\)|FLAGS.*NOW' "$stage/accepted.readelf.txt" >/dev/null ||
  fail "accepted fixture is not eager-bound"
grep -F 'GNU_RELRO' "$stage/accepted.readelf.txt" >/dev/null ||
  fail "accepted fixture has no RELRO"
grep -F 'GNU_EH_FRAME' "$stage/accepted.readelf.txt" >/dev/null ||
  fail "accepted fixture has no discoverable unwind header"
! grep -E '^  TLS |\((RELR|REL|INIT|PREINIT_ARRAY|RPATH|RUNPATH|TEXTREL|SYMBOLIC)\)' \
  "$stage/accepted.readelf.txt" | grep -Ev 'RELA|INIT_ARRAY' >/dev/null ||
  fail "accepted fixture acquired an unsupported ELF capability"

relocation_count="$(grep -c 'R_AARCH64_' "$stage/accepted.readelf.txt")"
[[ "$relocation_count" == "$EXPECTED_STATIC_UNWIND_RELOCATIONS" ]] ||
  fail "accepted fixture relocation count drift: $relocation_count"
for expected in '19 R_AARCH64_RELATIVE' '2 R_AARCH64_ABS64' \
  '9 R_AARCH64_GLOB_DAT' '28 R_AARCH64_JUMP_SLOT'; do
  actual="$(grep -o 'R_AARCH64_[A-Z0-9_]*' "$stage/accepted.readelf.txt" | \
    LC_ALL=C sort | uniq -c | awk '{$1=$1; print}' | grep -F "$expected" || true)"
  [[ "$actual" == "$expected" ]] || fail "accepted relocation drift: $expected"
done

[[ "$("$nm" -a -S "$accepted" | grep -c ' 0000000000000084 t _Unwind_Resume$')" == 1 ]] ||
  fail "local _Unwind_Resume implementation drift"
[[ "$("$nm" -a -S "$accepted" | grep -c ' 0000000000000028 t __unw_resume$')" == 1 ]] ||
  fail "local __unw_resume implementation drift"
[[ "$("$nm" -D --defined-only "$accepted" | grep -c ' T aim_libcxx_exception$')" == 1 ]] ||
  fail "exception export missing"
[[ "$("$nm" -D --defined-only "$accepted" | grep -c ' T JNI_OnLoad$')" == 1 ]] ||
  fail "self-testing JNI_OnLoad export missing"

# Source-derived check for the only Linux syscall shape used by the pinned
# libunwind address-readability probe. Its facade gate covers EINVAL/EFAULT.
unwind_source="$root/_aosp/bionic-syscall-facade/libunwind/src/UnwindCursor.hpp"
grep -F 'SYS_rt_sigprocmask, /*how=*/~0, sigsetAddr, nullptr, kernelSigsetSize' \
  "$unwind_source" >/dev/null || fail "libunwind readability probe source drift"

echo "android35-libcxx-exception-acceptance: PASS plain-blocker=_Unwind_Resume@LIBC_R accepted=static-android-libunwind imports=closed expected=$EXPECTED_RESULT execution=deferred-to-real-providers"
