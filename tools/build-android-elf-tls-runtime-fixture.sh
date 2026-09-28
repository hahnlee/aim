#!/bin/bash
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
source_file="$root/probes/android-elf-tls-fixture/tls_runtime.c"
output_dir="$root/_build/android-elf-tls-runtime-fixture"
lock_file="$root/upstream/android35-elf-tls-runtime.lock"
source "$lock_file"
sdk_root="${ANDROID_SDK_ROOT:-${ANDROID_HOME:-/Users/${USER:?USER is required}/Library/Android/sdk}}"
ndk_root="${ANDROID_NDK_ROOT:-${ANDROID_NDK_HOME:-$sdk_root/ndk/$NDK_REVISION}}"

fail() {
  echo "android-elf-tls-runtime-fixture: $*" >&2
  exit 1
}

[[ "$(basename "$ndk_root")" == "$NDK_REVISION" ]] ||
  fail "Android NDK is not pinned r28c: $ndk_root"
toolchain="$ndk_root/toolchains/llvm/prebuilt/darwin-x86_64"
clang="$toolchain/bin/aarch64-linux-android35-clang"
readelf="$toolchain/bin/llvm-readelf"
libc="$toolchain/sysroot/usr/lib/aarch64-linux-android/35/libc.so"
for input in "$clang" "$readelf" "$libc" "$source_file"; do
  [[ -e "$input" ]] || fail "missing pinned input: $input"
done
sha() { shasum -a 256 "$1" | awk '{print $1}'; }
[[ "$(sha "$ndk_root/source.properties")" == "$NDK_SOURCE_PROPERTIES_SHA256" ]] ||
  fail "NDK source.properties hash drift"
[[ "$(sha "$source_file")" == "$FIXTURE_SOURCE_SHA256" ]] ||
  fail "fixture source hash drift"

stage="$(mktemp -d "${TMPDIR:-/tmp}/aim-tls-runtime.XXXXXX")"
trap 'rm -rf "$stage"' EXIT
output="$stage/libaim_tls_runtime.so"
"$clang" -shared -fPIC -O2 -fvisibility=hidden -fno-stack-protector \
  -nostdlib -nodefaultlibs -fuse-ld=lld \
  -Wl,-soname,libaim_tls_runtime.so \
  -Wl,-z,now,-z,relro,--hash-style=sysv,--build-id=none \
  -Wl,-z,max-page-size=16384,-z,common-page-size=16384 \
  "$source_file" -Wl,--no-as-needed "$libc" -Wl,--as-needed -o "$output"
[[ "$(stat -f '%z' "$output")" == "$FIXTURE_ELF_SIZE" ]] ||
  fail "fixture ELF size drift"
[[ "$(sha "$output")" == "$FIXTURE_ELF_SHA256" ]] ||
  fail "fixture ELF hash drift"

"$readelf" -l -d -r -V --dyn-syms "$output" > "$stage/readelf.txt"
grep -E 'TLS.*0x40' "$stage/readelf.txt" >/dev/null ||
  fail "PT_TLS alignment is not 64 bytes"
[[ "$(grep -c 'R_AARCH64_TLSDESC' "$stage/readelf.txt")" == 3 ]] ||
  fail "expected three local TLSDESC relocations"
grep -E 'File: libc\.so' "$stage/readelf.txt" >/dev/null ||
  fail "libc version provider is missing"
for symbol in pthread_create pthread_join; do
  grep -E "UND.*${symbol}@LIBC" "$stage/readelf.txt" >/dev/null ||
    fail "missing exact ${symbol}@LIBC import"
done
if grep -E '(GLOBAL|WEAK).*UND ' "$stage/readelf.txt" |
  grep -Ev 'pthread_(create|join)@LIBC| _DYNAMIC$' >/dev/null; then
  fail "unexpected undefined import"
fi

mkdir -p "$output_dir"
cp "$output" "$output_dir/libaim_tls_runtime.so"
echo "android-elf-tls-runtime-fixture: PASS TLS=local-TLSDESC pthreads=4 align=64 imports=2@LIBC"
