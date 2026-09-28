#!/bin/bash
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
here="$root/tools/android35-libcxx-acceptance"
source "$here/sources.lock"

fail() {
  echo "android35-libcxx-acceptance: $*" >&2
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
for input in "$clang" "$readelf" "$nm" "$libcxx" "$libc"; do
  [[ -e "$input" ]] || fail "missing pinned input: $input"
done

hash_file() {
  shasum -a 256 "$1" | awk '{print $1}'
}

[[ "$(hash_file "$libcxx")" == "$LIBCXX_SHA256" ]] || fail "libc++ hash drift"
[[ "$(stat -f '%z' "$libcxx")" == "$LIBCXX_EXPECTED_SIZE" ]] ||
  fail "libc++ size drift"
[[ "$(hash_file "$here/consumer.cc")" == "$CONSUMER_SOURCE_SHA256" ]] ||
  fail "consumer source hash drift"
[[ "$(hash_file "$here/consumer.imports")" == "$CONSUMER_IMPORTS_SHA256" ]] ||
  fail "consumer import lock drift"

stage="$(mktemp -d "${TMPDIR:-/tmp}/aim-libcxx-acceptance.XXXXXX")"
trap 'rm -rf "$stage"' EXIT
consumer="$stage/libaim_libcxx_consumer.so"

"$clang" -shared -fPIC -O2 -fvisibility=hidden -fno-exceptions -fno-rtti \
  -nostdlib -nodefaultlibs -Wl,-soname,libaim_libcxx_consumer.so \
  -Wl,-z,now,-z,relro,--hash-style=sysv -Wl,--build-id=none \
  "$here/consumer.cc" -Wl,--no-as-needed "$libcxx" "$libc" -Wl,--as-needed \
  -o "$consumer"

[[ "$(hash_file "$consumer")" == "$CONSUMER_ELF_SHA256" ]] ||
  fail "consumer ELF hash drift"
file "$consumer" | grep -F 'ELF 64-bit LSB shared object, ARM aarch64' >/dev/null ||
  fail "consumer is not Android AArch64 ELF"

"$readelf" -l -d -r -V --dyn-syms --wide "$consumer" > "$stage/consumer.readelf.txt"
[[ "$(grep -c '(NEEDED)' "$stage/consumer.readelf.txt")" == 2 ]] ||
  fail "consumer DT_NEEDED set drift"
grep -E '\(NEEDED\).*\[libc\+\+_shared\.so\]' "$stage/consumer.readelf.txt" >/dev/null ||
  fail "consumer does not require real libc++_shared.so"
grep -E '\(NEEDED\).*\[libc\.so\]' "$stage/consumer.readelf.txt" >/dev/null ||
  fail "compiler-emitted libc calls lack Android libc ownership"
grep -E 'memcpy@LIBC' "$stage/consumer.readelf.txt" >/dev/null ||
  fail "compiler-emitted memcpy lacks its Android LIBC version requirement"
grep -E '\(SONAME\).*\[libaim_libcxx_consumer\.so\]' \
  "$stage/consumer.readelf.txt" >/dev/null || fail "consumer SONAME drift"
grep -E '\(BIND_NOW\)|FLAGS.*NOW' "$stage/consumer.readelf.txt" >/dev/null ||
  fail "consumer is not eager-bound"
grep -F 'GNU_RELRO' "$stage/consumer.readelf.txt" >/dev/null ||
  fail "consumer has no GNU RELRO"
! grep -E '^  TLS |\((RELR|REL|INIT|PREINIT_ARRAY|RPATH|RUNPATH|TEXTREL|SYMBOLIC)\)' \
  "$stage/consumer.readelf.txt" | grep -Ev 'RELA|INIT_ARRAY' >/dev/null ||
  fail "consumer acquired an unsupported ELF capability"
[[ "$(grep -c 'R_AARCH64_JUMP_SLOT' "$stage/consumer.readelf.txt")" == 12 ]] ||
  fail "consumer relocation set drift"
! grep -E 'R_AARCH64_(RELATIVE|ABS64|GLOB_DAT)' "$stage/consumer.readelf.txt" >/dev/null ||
  fail "consumer acquired an unexpected non-PLT relocation"

"$nm" -D --undefined-only "$consumer" | awk '{print $2}' | LC_ALL=C sort \
  > "$stage/consumer.imports"
cmp -s "$here/consumer.imports" "$stage/consumer.imports" ||
  fail "consumer import manifest drift"
[[ "$("$nm" -D --defined-only "$consumer" | grep -c ' T aim_libcxx_collections$')" == 1 ]] ||
  fail "consumer export missing"
[[ "$("$nm" -D --defined-only "$consumer" | grep -c ' T JNI_OnLoad$')" == 1 ]] ||
  fail "self-testing JNI_OnLoad export missing"

"$readelf" -l -d -r -V --dyn-syms --wide "$libcxx" > "$stage/libcxx.readelf.txt"
for needed in libc.so libm.so libdl.so; do
  grep -E "\\(NEEDED\\).*\\[$needed\\]" "$stage/libcxx.readelf.txt" >/dev/null ||
    fail "libc++ dependency drift: $needed"
done
[[ "$(grep -c '(NEEDED)' "$stage/libcxx.readelf.txt")" == 3 ]] ||
  fail "libc++ DT_NEEDED count drift"
grep -E '\(AARCH64_BTI_PLT\).*0$' "$stage/libcxx.readelf.txt" >/dev/null ||
  fail "libc++ BTI PLT tag drift"
grep -F 'GNU_RELRO' "$stage/libcxx.readelf.txt" >/dev/null || fail "libc++ lost RELRO"
grep -E '\(BIND_NOW\)|FLAGS.*NOW' "$stage/libcxx.readelf.txt" >/dev/null ||
  fail "libc++ lost eager binding"
! grep -E '^  TLS |\((RELR|REL|INIT|PREINIT_ARRAY|RPATH|RUNPATH|TEXTREL|SYMBOLIC)\)' \
  "$stage/libcxx.readelf.txt" | grep -Ev 'RELA|INIT_ARRAY' >/dev/null ||
  fail "libc++ acquired an unsupported ELF capability"

relocation_count="$(grep -c 'R_AARCH64_' "$stage/libcxx.readelf.txt")"
[[ "$relocation_count" == "$LIBCXX_EXPECTED_RELOCATIONS" ]] ||
  fail "libc++ relocation count drift: $relocation_count"
for expected in '1587 R_AARCH64_RELATIVE' '2012 R_AARCH64_ABS64' \
  '189 R_AARCH64_GLOB_DAT' '479 R_AARCH64_JUMP_SLOT'; do
  actual="$(grep -o 'R_AARCH64_[A-Z0-9_]*' "$stage/libcxx.readelf.txt" | \
    LC_ALL=C sort | uniq -c | awk '{$1=$1; print}' | grep -F "$expected" || true)"
  [[ "$actual" == "$expected" ]] || fail "libc++ relocation distribution drift: $expected"
done

strong_imports="$("$readelf" --dyn-syms --wide "$libcxx" | \
  awk '$7 == "UND" && ($5 == "GLOBAL") && ($4 == "FUNC" || $4 == "OBJECT") {count++} END {print count+0}')"
weak_imports="$("$readelf" --dyn-syms --wide "$libcxx" | \
  awk '$7 == "UND" && $5 == "WEAK" && $8 != "" {count++} END {print count+0}')"
[[ "$strong_imports" == "$LIBCXX_EXPECTED_IMPORTS" ]] ||
  fail "libc++ strong import count drift: $strong_imports"
[[ "$weak_imports" == "$LIBCXX_EXPECTED_WEAK_IMPORTS" ]] ||
  fail "libc++ weak import count drift: $weak_imports"

# This loader gate performs the non-executing proof: the real 9 MiB image is
# mapped, all 4,267 relocations and 160 @LIBC requests are applied, RELRO is
# sealed, an export is found, and malformed BTI tags are rejected. It does not
# run libc++ constructors against placeholder provider addresses.
"$root/crates/aim-elf-loader/run-gate.sh" >/dev/null

echo "android35-libcxx-acceptance: PASS structural=real-libcxx consumer=string+vector+sort+filesystem-copy expected=$CONSUMER_EXPECTED_RESULT execution=deferred-to-real-providers"
