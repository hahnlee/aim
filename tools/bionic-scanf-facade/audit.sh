#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd)"
repo_root="$(cd "$script_dir/../.." && pwd)"
source_root="$repo_root/_aosp/bionic-scanf-facade"
build_root="$repo_root/_build/bionic-scanf-facade"
BIONIC_REVISION=361ba86734fb2821a6adcfdf775db8abd04e0de0
NDK_REVISION=28.2.13676358
ANDROID_API=35

fail() { echo "bionic-scanf-facade: $*" >&2; exit 1; }
sha() { shasum -a 256 "$1" | awk '{print $1}'; }
check_hash() { [[ "$(sha "$1")" == "$2" ]] || fail "hash drift: $1"; }
bash -n "$script_dir/audit.sh"
# shellcheck disable=SC1091
source "$script_dir/sources.lock"
check_hash "$script_dir/audit.sh" "$AUDIT_SHA256"
[[ "$BIONIC_REVISION" == 361ba86734fb2821a6adcfdf775db8abd04e0de0 ]] || fail 'Bionic revision drift'
[[ "$NDK_REVISION" == 28.2.13676358 && "$ANDROID_API" == 35 ]] || fail 'NDK revision drift'
check_hash "$script_dir/upstream-sources.tsv" "$UPSTREAM_SOURCES_SHA256"
check_hash "$script_dir/manifests/demand.tsv" "$DEMAND_SHA256"
check_hash "$script_dir/manifests/imports.tsv" "$IMPORTS_SHA256"
check_hash "$script_dir/manifests/libcxx-format-corpus.tsv" "$CORPUS_SHA256"
for pair in \
  "include/aim_bionic_scanf.h:$HEADER_SHA256" \
  "src/scanf.cc:$SCANF_SHA256" "src/aapcs64_entry.S:$ENTRY_SHA256" \
  "src/main.rs:$MAIN_SHA256" "probes/fixture.c:$FIXTURE_SHA256" \
  "probes/exports.map:$EXPORTS_SHA256" "build.rs:$BUILD_RS_SHA256" \
  "Cargo.toml:$CARGO_TOML_SHA256" \
  "README.md:$README_SHA256"; do
  check_hash "$script_dir/${pair%%:*}" "${pair#*:}"
done

while IFS=$'\t' read -r relative size digest; do
  [[ "$relative" != path ]] || continue
  destination="$source_root/$relative"
  if [[ ! -f "$destination" || "$(stat -f %z "$destination")" != "$size" || "$(sha "$destination")" != "$digest" ]]; then
    mkdir -p "$(dirname "$destination")"
    encoded="$(mktemp)"; trap 'rm -f "$encoded"' EXIT
    curl -fsSL "https://android.googlesource.com/platform/bionic/+/$BIONIC_REVISION/$relative?format=TEXT" -o "$encoded"
    base64 -D < "$encoded" > "$destination"
    rm -f "$encoded"; trap - EXIT
  fi
  [[ "$(stat -f %z "$destination")" == "$size" && "$(sha "$destination")" == "$digest" ]] || fail "source drift: $relative"
done < "$script_dir/upstream-sources.tsv"
[[ -z "$(find "$source_root" \( -name .git -o -name .gitmodules \) -print -quit)" ]] || fail 'git metadata forbidden'

python3 - "$source_root" "$script_dir" <<'PY'
import sys
from pathlib import Path
r, s = map(Path, sys.argv[1:])
stdio=(r/'libc/stdio/stdio.cpp').read_text(); vf=(r/'libc/stdio/vfscanf.cpp').read_text(); common=(r/'libc/stdio/scanf_common.h').read_text(); vs=(r/'libc/upstream-openbsd/lib/libc/stdio/vsscanf.c').read_text()
assert 'int sscanf(const char* s, const char* fmt, ...)' in stdio and 'vsscanf(s, fmt, ap)' in stdio
assert 'int __svfscanf(FILE* fp, const char* fmt0, va_list ap)' in vf
for token in ["case 'b':", "case 'w':", "case 'n':", 'case CT_FLOAT:', 'case CT_CCL:', 'case CT_STRING:', 'case CT_CHAR:']:
    assert token in vf
assert '#define LONGDBL' in common and '#define ALLOCATE' in common and 'return (__svfscanf(&f, fmt, ap));' in vs
ours=(s/'src/scanf.cc').read_text()
for forbidden in ['::sscanf(', '::vsscanf(', '::vfscanf(', 'std::fscanf(', 'long double']:
    assert forbidden not in ours
assert 'aim_bionic_stdio_scan_core' in ours
print('source-contract=PASS Bionic-vfscanf-model provider-FILE-lease')
PY

ndk_root="${ANDROID_NDK_ROOT:-$HOME/Library/Android/sdk/ndk/$NDK_REVISION}"
toolchain="$ndk_root/toolchains/llvm/prebuilt/darwin-x86_64/bin"
android_cc="$toolchain/aarch64-linux-android${ANDROID_API}-clang"
readelf="$toolchain/llvm-readelf"; objdump="$toolchain/llvm-objdump"; nm_android="$toolchain/llvm-nm"
for tool in "$android_cc" "$readelf" "$objdump" "$nm_android"; do [[ -x "$tool" ]] || fail "missing $tool"; done
check_hash "$ndk_root/source.properties" "$NDK_SOURCE_PROPERTIES_SHA256"
libcxx="$ndk_root/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/lib/aarch64-linux-android/libc++_shared.so"
check_hash "$libcxx" "$NDK_LIBCXX_SHA256"; [[ "$(stat -f %z "$libcxx")" == 9236352 ]] || fail 'libc++ size drift'
locale="$ndk_root/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include/c++/v1/locale"
fallback="$ndk_root/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include/c++/v1/__locale_dir/locale_base_api/bsd_locale_fallbacks.h"
check_hash "$locale" "$NDK_LOCALE_SHA256"; check_hash "$fallback" "$NDK_FALLBACK_SHA256"
undefined="$($nm_android -D -u "$libcxx")"
for symbol in sscanf vsscanf; do grep -E " U ${symbol}@LIBC$" <<< "$undefined" >/dev/null || fail "missing libc++ demand: $symbol"; done
grep -F '"%p"' "$locale" >/dev/null || fail 'missing %p corpus'; grep -F '"%Lf"' "$locale" >/dev/null || fail 'missing %Lf corpus'

mkdir -p "$build_root"
fixture="$build_root/libbionic_scanf_fixture.so"
"$android_cc" -std=c17 -O2 -Wall -Wextra -Werror -Wno-format -fno-builtin -fPIC -shared -nostdlib \
  -Wl,--no-undefined -Wl,--version-script,"$script_dir/probes/exports.map" -Wl,-z,norelro -Wl,--hash-style=both \
  "$script_dir/probes/fixture.c" -lc -o "$fixture"
elf_imports="$($readelf --dyn-syms --wide "$fixture")"
for symbol in __errno fclose fopen fscanf sscanf vsscanf; do grep -E " UND[[:space:]]+${symbol}@LIBC( |$)" <<< "$elf_imports" >/dev/null || fail "missing ELF import: $symbol"; done
[[ "$(grep -Ec ' UND[[:space:]]+[^ ]+@LIBC( |$)' <<< "$elf_imports")" == 6 ]] || fail 'unexpected fixture imports'
disassembly="$($objdump -d "$fixture")"; grep -E 'bl.*<fscanf' <<< "$disassembly" >/dev/null || fail 'no fscanf call'; grep -E 'bl.*<sscanf' <<< "$disassembly" >/dev/null || fail 'no sscanf call'; grep -E 'bl.*<vsscanf' <<< "$disassembly" >/dev/null || fail 'no vsscanf call'

cargo fmt --manifest-path "$script_dir/Cargo.toml" -- --check
CARGO_TARGET_DIR="$build_root/clippy" cargo clippy --quiet --all-targets --manifest-path "$script_dir/Cargo.toml" -- -D warnings
CARGO_TARGET_DIR="$build_root/target" cargo run --quiet --manifest-path "$script_dir/Cargo.toml" -- "$fixture"
archive="$(find "$build_root/target/debug/build" -path '*/out/libaim_bionic_scanf.a' -print | head -1)"
[[ -f "$archive" ]] || fail 'archive missing'; cp "$archive" "$build_root/libaim-bionic-scanf.a"
[[ "$(ar -t "$archive" | grep -v '^__.SYMDEF' | wc -l | tr -d ' ')" == 2 ]] || fail 'archive member drift'
definitions="$(nm -gU "$archive")"
for symbol in aim_bionic_fscanf aim_bionic_sscanf aim_bionic_vsscanf aim_bionic_scanf_resolve; do grep -F " _$symbol" <<< "$definitions" >/dev/null || fail "definition missing: $symbol"; done
for duplicate in aim_bionic_malloc aim_bionic___errno aim_bionic_strtod aim_bionic_strtold_raw android_icu_init; do
  grep -E " [TDS] _${duplicate}$" <<< "$definitions" >/dev/null && fail "duplicate owner: $duplicate"
done
archive_undefined="$(nm -u "$archive" | awk '{print $NF}' | sort -u)"
for edge in _aim_bionic_stdio_scan_core _aim_bionic_strtoll _aim_bionic_strtoull _aim_bionic_strtof _aim_bionic_strtod _aim_bionic_strtold_raw _aim_bionic_errno_store; do grep -Fx "$edge" <<< "$archive_undefined" >/dev/null || fail "provider edge missing: $edge"; done
if grep -E '^_(sscanf|vsscanf|vfscanf|strtod|strtof|strtold|wcstod|mbrtowc|iswspace)$' <<< "$archive_undefined" >/dev/null; then fail 'host scanf/wchar/float fallback'; fi
otool -tvV "$archive" | grep -E 'stp[[:space:]]+q6, q7, \[sp, #0xa0\]' >/dev/null || fail 'FP-bank capture missing'
otool -tvV "$archive" | grep -F '_aim_bionic_fscanf_captured' >/dev/null || fail 'fscanf AAPCS64 capture call missing'

for sanitizer in address undefined; do
  target="$build_root/cargo-$sanitizer"
  env BIONIC_SCANF_C_SANITIZER="$sanitizer" CARGO_TARGET_DIR="$target" cargo run --quiet --manifest-path "$script_dir/Cargo.toml" -- "$fixture" >/dev/null
done
runner="$build_root/target/debug/bionic-scanf-facade"
otool -L "$runner" | grep -Ei '(homebrew|/opt/|libicu|libandroidicu|quadmath)' >/dev/null && fail 'dynamic host fallback linked'
git -C "$repo_root" diff --check -- "$script_dir"
git -C "$repo_root" diff --cached --quiet -- "$script_dir" || fail 'staged standalone changes'
[[ ! -e "$script_dir/target" ]] || fail 'module-local target directory leaked'
echo 'bionic-scanf-facade: PASS demand=3 archive=2-members AndroidELF=fscanf+sscanf+vsscanf FILE=provider-cursor formats=int+float+binary128+string+char+scanset+n AAPCS64=GP+FP+stack va_list=32 ASan UBSan closed=libc.so@LIBC'
