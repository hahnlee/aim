#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd)"
repo_root="$(cd "$script_dir/../.." && pwd)"
source_root="$repo_root/_aosp/bionic-swprintf-facade"
build_root="$repo_root/_build/bionic-swprintf-facade"
# shellcheck disable=SC1091
source "$script_dir/sources.lock"

fail() { echo "bionic-swprintf-facade: $*" >&2; exit 1; }
sha() { shasum -a 256 "$1" | awk '{print $1}'; }
bash -n "$script_dir/audit.sh"

while IFS=$'\t' read -r relative size digest; do
  [[ "$relative" != path ]] || continue
  destination="$source_root/$relative"
  if [[ ! -f "$destination" ]]; then
    mkdir -p "$(dirname "$destination")"
    encoded="$(mktemp)"
    curl -fsSL "https://android.googlesource.com/platform/bionic/+/$BIONIC_REVISION/$relative?format=TEXT" -o "$encoded"
    base64 -D < "$encoded" > "$destination"
    rm -f "$encoded"
  fi
  [[ "$(stat -f %z "$destination")" == "$size" && "$(sha "$destination")" == "$digest" ]] ||
    fail "source drift: $relative"
done < "$script_dir/upstream-sources.tsv"
[[ -z "$(find "$source_root" \( -name .git -o -name .gitmodules \) -print -quit)" ]] ||
  fail 'Git metadata forbidden'

python3 - "$source_root" <<'PY'
import sys
from pathlib import Path
r=Path(sys.argv[1])
stdio=(r/'libc/stdio/stdio.cpp').read_text()
vsw=(r/'libc/upstream-openbsd/lib/libc/stdio/vswprintf.c').read_text()
gdtoa=(r/'libc/upstream-openbsd/lib/libc/gdtoa/gdtoa.c').read_text()
assert 'int swprintf(wchar_t* s, size_t n, const wchar_t* fmt, ...)' in stdio
assert 'PRINTF_IMPL(vswprintf(s, n, fmt, ap))' in stdio
assert 'errno = EOVERFLOW' in vsw and 'nwc == n' in vsw
assert '(FPI *fpi, int be, ULong *bits, int *kindp, int mode, int ndigits' in gdtoa
print('bionic-swprintf-facade: source-contract=PASS swprintf->vswprintf gdtoa-mode3')
PY

ndk_root="${ANDROID_NDK_ROOT:-$HOME/Library/Android/sdk/ndk/$NDK_REVISION}"
toolchain="$ndk_root/toolchains/llvm/prebuilt/darwin-x86_64/bin"
cc="$toolchain/aarch64-linux-android35-clang"
readelf="$toolchain/llvm-readelf"
objdump="$toolchain/llvm-objdump"
[[ -x "$cc" && -x "$readelf" && -x "$objdump" ]] || fail 'pinned NDK tools missing'
[[ "$(sha "$ndk_root/source.properties")" == "$NDK_SOURCE_PROPERTIES_SHA256" ]] || fail 'NDK source drift'
libcxx="$ndk_root/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/lib/aarch64-linux-android/libc++_shared.so"
[[ "$(sha "$libcxx")" == "$NDK_LIBCXX_SHA256" ]] || fail 'libc++ drift'
[[ "$("$readelf" --dyn-syms --wide "$libcxx" | grep -Ec ' UND[[:space:]]+swprintf@LIBC( |$)')" == 1 ]] ||
  fail 'libc++ swprintf demand drift'
calls="$("$objdump" -d --demangle --no-show-raw-insn "$libcxx" | grep -c '<swprintf@plt>')"
[[ "$calls" -ge 4 ]] || fail 'libc++ swprintf callsite drift'

mkdir -p "$build_root"
fixture="$build_root/libfixture.so"
"$cc" -std=c17 -O2 -Wall -Wextra -Werror -fno-builtin -fPIC -shared -nostdlib \
  -Wl,--no-undefined -Wl,--version-script,"$script_dir/probes/exports.map" \
  -Wl,-z,norelro -Wl,--hash-style=both "$script_dir/probes/fixture.c" -lc -o "$fixture"
imports="$("$readelf" --dyn-syms --wide "$fixture")"
[[ "$(grep -Ec ' UND[[:space:]]+swprintf@LIBC( |$)' <<< "$imports")" == 1 ]] || fail 'fixture import drift'
[[ "$(grep -Ec ' UND[[:space:]]+[^ ]+@LIBC( |$)' <<< "$imports")" == 1 ]] || fail 'unexpected fixture import'
"$objdump" -d "$fixture" | grep -E 'bl.*<swprintf' >/dev/null || fail 'AAPCS64 swprintf call absent'

cargo fmt --manifest-path "$script_dir/Cargo.toml" -- --check
CARGO_TARGET_DIR="$build_root/clippy" cargo clippy --quiet --all-targets --manifest-path "$script_dir/Cargo.toml" -- -D warnings
CARGO_TARGET_DIR="$build_root/target" cargo run --quiet --manifest-path "$script_dir/Cargo.toml" -- "$fixture"
for sanitizer in address undefined; do
  UBSAN_OPTIONS=halt_on_error=1 BIONIC_SWPRINTF_C_SANITIZER="$sanitizer" CARGO_TARGET_DIR="$build_root/$sanitizer" \
    cargo run --quiet --manifest-path "$script_dir/Cargo.toml" -- "$fixture" >/dev/null
done

archive="$(find "$build_root/target/debug/build" -path '*/out/libaim_bionic_swprintf.a' -print | head -1)"
[[ -f "$archive" ]] || fail 'archive missing'
cp "$archive" "$build_root/libaim-bionic-swprintf.a"
[[ "$(ar -t "$archive" | grep -v '^__.SYMDEF' | wc -l | tr -d ' ')" == 3 ]] || fail 'archive member drift'
undefined="$(nm -u "$archive" | awk '{print $NF}' | sort -u)"
for edge in _aim_aosp_gdtoa _aim_bionic_vsnprintf _aim_bionic_errno_store ___freedtoa; do
  grep -Fx "$edge" <<< "$undefined" >/dev/null || fail "missing provider edge: $edge"
done
if grep -E '^_(swprintf|vswprintf|sprintf|snprintf|wcrtomb|mbrtowc)$' <<< "$undefined" >/dev/null; then
  fail 'host wide/format fallback'
fi
git -C "$repo_root" diff --check -- "$script_dir"
[[ ! -e "$script_dir/target" ]] || fail 'module-local target directory leaked'
echo 'bionic-swprintf-facade: PASS demand=1 AndroidELF=%f+%Lf AAPCS64=v0-q0 wchar32 binary128=gdtoa mode3 C-ASan C-UBSan closed=libc.so@LIBC target-clean'
