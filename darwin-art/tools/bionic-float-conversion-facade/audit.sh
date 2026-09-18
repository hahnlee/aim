#!/bin/bash
set -euo pipefail
export LC_ALL=C

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/../.." && pwd)"
source "$script_dir/sources.lock"

fail() { echo "bionic-float-conversion-facade: $*" >&2; exit 3; }
missing() { echo "bionic-float-conversion-facade: missing $*" >&2; exit 2; }
sha() { shasum -a 256 "$1" | awk '{print $1}'; }
check_hash() { [[ "$(sha "$1")" == "$2" ]] || fail "hash mismatch: $1"; }

master="$project_root/tools/bionic-libc-leaf-facade/imports/ndk-r28c-api35-arm64-libc.tsv"
errno_root="$project_root/tools/bionic-errno-tls"
check_hash "$master" "$LIBC_IMPORT_MANIFEST_SHA256"
check_hash "$errno_root/sources.lock" "$BIONIC_ERRNO_LOCK_SHA256"
check_hash "$errno_root/src/errno_tls.c" "$BIONIC_ERRNO_SOURCE_SHA256"
check_hash "$errno_root/generated/darwin_to_android.inc" "$BIONIC_ERRNO_MAPPING_SHA256"
check_hash "$script_dir/upstream-sources.tsv" "$UPSTREAM_SOURCES_SHA256"
check_hash "$script_dir/manifests/demand.tsv" "$DEMAND_SHA256"
check_hash "$script_dir/manifests/imports.tsv" "$IMPORTS_SHA256"
check_hash "$script_dir/include/darwin_art_gdtoa_compat.h" "$COMPAT_SHA256"
check_hash "$script_dir/include/thread_private.h" "$THREAD_PRIVATE_SHA256"
check_hash "$script_dir/include/darwin_art_bionic_float_conversion.h" "$HEADER_SHA256"
check_hash "$script_dir/src/provider.cc" "$PROVIDER_SHA256"
check_hash "$script_dir/probes/host_state.cc" "$HOST_STATE_HELPER_SHA256"
check_hash "$script_dir/src/lock_adapter.cc" "$LOCK_ADAPTER_SHA256"
check_hash "$script_dir/src/main.rs" "$MAIN_SHA256"
check_hash "$script_dir/probes/fixture.c" "$FIXTURE_SHA256"
check_hash "$script_dir/probes/abi.c" "$ABI_SHA256"
check_hash "$script_dir/probes/differential.cc" "$DIFFERENTIAL_SHA256"
check_hash "$script_dir/probes/exports.map" "$EXPORTS_SHA256"
check_hash "$script_dir/build.rs" "$BUILD_RS_SHA256"
check_hash "$script_dir/Cargo.toml" "$CARGO_TOML_SHA256"
check_hash "$script_dir/README.md" "$README_SHA256"

temp_root="$(mktemp -d "${TMPDIR:-/tmp}/bionic-float-conversion.XXXXXX")"
cleanup() {
  if [[ "$temp_root" == "${TMPDIR:-/tmp}"/bionic-float-conversion.* ]]; then
    find "$temp_root" -depth -delete
  fi
}
trap cleanup EXIT

awk -F '\t' 'NR>1 && ($1=="strtod" || $1=="strtof" || $1=="strtold" || $1=="strtold_l") {print $1 "\t" $2 "\t" $3}' \
  "$master" | sort > "$temp_root/master-demand"
tail -n +2 "$script_dir/manifests/demand.tsv" | cut -f1-3 | sort \
  > "$temp_root/locked-demand"
diff -u "$temp_root/master-demand" "$temp_root/locked-demand" ||
  fail 'pinned libc++ floating conversion demand drift'
[[ "$(wc -l < "$temp_root/master-demand" | tr -d ' ')" == 4 ]] ||
  fail 'floating conversion demand count drift'
awk -F '\t' 'NR>1 && $4=="supported" {print $1}' \
  "$script_dir/manifests/demand.tsv" | sort > "$temp_root/supported"
tail -n +2 "$script_dir/manifests/imports.tsv" | cut -f1 | sort \
  > "$temp_root/imports"
diff -u "$temp_root/supported" "$temp_root/imports" ||
  fail 'supported/import manifest drift'
[[ "$(awk -F '\t' 'NR>1 && $4=="supported" {n++} END {print n+0}' "$script_dir/manifests/demand.tsv")" == 2 ]] ||
  fail 'supported conversion count drift'
[[ "$(awk -F '\t' 'NR>1 && $4=="rejected" {n++} END {print n+0}' "$script_dir/manifests/demand.tsv")" == 2 ]] ||
  fail 'rejected conversion count drift'

source_root="$project_root/_aosp/bionic-float-conversion-facade"
[[ "$(tail -n +2 "$script_dir/upstream-sources.tsv" | wc -l | tr -d ' ')" == "$BIONIC_SOURCE_COUNT" ]] ||
  fail 'upstream source count drift'
while IFS=$'\t' read -r relative size expected; do
  [[ "$relative" != path ]] || continue
  destination="$source_root/$relative"
  if [[ ! -f "$destination" ]]; then
    mkdir -p "$(dirname "$destination")"
    staged="$(mktemp "${TMPDIR:-/tmp}/bionic-float-source.XXXXXX")"
    curl -fsSL "https://android.googlesource.com/$BIONIC_PROJECT/+/$BIONIC_REVISION/$relative?format=TEXT" |
      base64 -D > "$staged"
    [[ "$(stat -f %z "$staged")" == "$size" && "$(sha "$staged")" == "$expected" ]] ||
      fail "download provenance mismatch: $relative"
    mv "$staged" "$destination"
  fi
  [[ "$(stat -f %z "$destination")" == "$size" && "$(sha "$destination")" == "$expected" ]] ||
    fail "sparse source drift: $relative"
done < "$script_dir/upstream-sources.tsv"
printf '%s\n' "$BIONIC_REVISION" > "$source_root/.source-revision"
[[ -z "$(find "$source_root" \( -name .git -o -name .gitmodules \) -print -quit)" ]] ||
  fail 'Git metadata forbidden in sparse source'

python3 - "$source_root" <<'PY'
import sys
from pathlib import Path
r=Path(sys.argv[1])
bp=(r/'libc/Android.bp').read_text()
strtod=(r/'libc/upstream-openbsd/lib/libc/gdtoa/strtod.c').read_text()
strtof=(r/'libc/upstream-openbsd/lib/libc/gdtoa/strtof.c').read_text()
strtold=(r/'libc/bionic/strtold.cpp').read_text()
stdlib_l=(r/'libc/bionic/stdlib_l.cpp').read_text()
tests=(r/'tests/stdlib_test.cpp').read_text()
support=(r/'libc/upstream-openbsd/android/gdtoa_support.cpp').read_text()
thread_private=(r/'libc/private/thread_private.h').read_text()
for source in ('dmisc.c','gethex.c','gmisc.c','hd_init.c','hexnan.c','misc.c',
               'smisc.c','strtod.c','strtodg.c','strtof.c','strtord.c','sum.c','ulp.c'):
    assert f'upstream-openbsd/lib/libc/gdtoa/{source}' in bp
assert 'INFNAN_CHECK' in (r/'libc/upstream-openbsd/android/include/arith.h').read_text()
assert "if (c == '.')" in strtod and 'errno = ERANGE' in strtod
assert 'gethex(&s' in strtod and 'hexnan(&s' in strtod
assert 'k = strtodg(s, sp, fpi, &exp, bits);' in strtof
assert '__strtorQ(s, end_ptr, FLT_ROUNDS, &result)' in strtold
assert 'long double strtold_l' in stdlib_l and 'return strtold(s, end_ptr);' in stdlib_l
assert 'double strtod_l' in stdlib_l and 'return strtod(s, end_ptr);' in stdlib_l
assert 'float strtof_l' in stdlib_l and 'return strtof(s, end_ptr);' in stdlib_l
assert 'pthread_mutex_t __dtoa_locks[]' in support
assert '#define _MUTEX_LOCK(l) pthread_mutex_lock((pthread_mutex_t*) l)' in thread_private
for anchor in ('+nan(0xff)', '0x1.2p3', '7.0064923216240853546186479164495e-46',
               '2.2250738585072012e-308'):
    assert anchor in tests
print('bionic-float-conversion-facade: upstream=PASS gdtoa-parser-sources=13 demand=4 support=2 reject=binary128-2')
PY

ndk_root="${ANDROID_NDK_ROOT:-$HOME/Library/Android/sdk/ndk/$NDK_REVISION}"
toolchain="$ndk_root/toolchains/llvm/prebuilt/darwin-x86_64/bin"
android_cc="$toolchain/aarch64-linux-android${ANDROID_API}-clang"
readelf="$toolchain/llvm-readelf"
elf_nm="$toolchain/llvm-nm"
ndk_stdlib="$ndk_root/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include/stdlib.h"
ndk_libc="$ndk_root/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/lib/aarch64-linux-android/$ANDROID_API/libc.so"
for input in "$ndk_root/source.properties" "$ndk_stdlib" "$ndk_libc"; do
  [[ -f "$input" ]] || missing "$input"
done
check_hash "$ndk_root/source.properties" "$NDK_SOURCE_PROPERTIES_SHA256"
check_hash "$ndk_stdlib" "$NDK_STDLIB_H_SHA256"
check_hash "$ndk_libc" "$NDK_API35_ARM64_LIBC_SHA256"
"$android_cc" -std=c17 -Wall -Wextra -Werror -fsyntax-only "$script_dir/probes/abi.c"

sdk="$(xcrun --sdk macosx --show-sdk-path)"
host_cc="$(xcrun --find clang)"
host_cxx="$(xcrun --find clang++)"
gdtoa_root="$source_root/libc/upstream-openbsd/lib/libc/gdtoa"
android_include="$source_root/libc/upstream-openbsd/android/include"
gdtoa_sources=(dmisc.c gethex.c gmisc.c hd_init.c hexnan.c misc.c smisc.c strtod.c strtodg.c strtof.c strtord.c sum.c ulp.c)

build_objects() {
  local mode="$1" output_dir="$2"
  local sanitizer=()
  if [[ "$mode" == sanitized ]]; then
    # Pinned OpenBSD gethex.c line 229 intentionally shifts a signed nibble
    # into bit 31. Android relies on the resulting 32-bit pattern; Clang's
    # shift checker diagnoses the source expression before that conversion.
    sanitizer=(-fsanitize=address,undefined -fno-sanitize=shift -fno-omit-frame-pointer)
  fi
  mkdir -p "$output_dir"
  built_objects=()
  for source in "${gdtoa_sources[@]}"; do
    object="$output_dir/${source%.c}.o"
    extra=()
    [[ "$source" != misc.c ]] || extra=(-DDARWIN_ART_GDTOA_LOCAL_LOCKS)
    "$host_cc" -arch arm64 -isysroot "$sdk" -std=gnu17 -O2 \
      -Wall -Wextra -Werror -Wno-sign-compare -fvisibility=hidden \
      "${sanitizer[@]+"${sanitizer[@]}"}" -I"$script_dir/include" -I"$android_include" \
      -I"$gdtoa_root" -include darwin_art_gdtoa_compat.h \
      -Dstrtod=darwin_art_aosp_strtod -Dstrtof=darwin_art_aosp_strtof \
      "${extra[@]+"${extra[@]}"}" -c "$gdtoa_root/$source" -o "$object"
    built_objects+=("$object")
  done
  "$host_cxx" -arch arm64 -isysroot "$sdk" -std=c++20 -O2 -Wall -Wextra \
    -Werror -fvisibility=hidden "${sanitizer[@]+"${sanitizer[@]}"}" -I"$script_dir/include" \
    -c "$script_dir/src/provider.cc" -o "$output_dir/provider.o"
  "$host_cxx" -arch arm64 -isysroot "$sdk" -std=c++20 -O2 -Wall -Wextra \
    -Werror -fvisibility=hidden "${sanitizer[@]+"${sanitizer[@]}"}" -I"$script_dir/include" \
    -c "$script_dir/src/lock_adapter.cc" -o "$output_dir/lock_adapter.o"
  "$host_cc" -arch arm64 -isysroot "$sdk" -std=c17 -O2 -Wall -Wextra \
    -Werror "${sanitizer[@]+"${sanitizer[@]}"}" -I"$errno_root/include" \
    -I"$errno_root/generated" -c "$errno_root/src/errno_tls.c" \
    -o "$output_dir/errno.o"
  built_objects+=("$output_dir/provider.o" "$output_dir/lock_adapter.o" "$output_dir/errno.o")
}

build_objects normal "$temp_root/normal"
normal_objects=("${built_objects[@]}")
ar rcs "$temp_root/libdarwin-art-bionic-float-conversion.a" "${normal_objects[@]}"
definitions="$(nm -gU "$temp_root/libdarwin-art-bionic-float-conversion.a")"
for symbol in strtod strtod_l strtof strtof_l float_conversion_resolve float_conversion_capability; do
  grep -F " _darwin_art_bionic_$symbol" <<< "$definitions" >/dev/null ||
    fail "missing provider definition: $symbol"
done
if grep -E ' (_strtod|_strtof|_strtold)$' <<< "$definitions" >/dev/null; then
  fail 'unprefixed host-interposing strto definition escaped'
fi
if rg -n '(std::)?strto(d|f|ld)\(' "$script_dir/src" "$script_dir/include" |
   grep -v 'darwin_art_.*strto' >/dev/null; then
  fail 'Darwin strto forwarding entered provider'
fi
[[ "$(nm -u "$temp_root/normal/provider.o" | awk '{print $NF}' | grep -Ec '^_darwin_art_aosp_strto(d|f)$')" == 2 ]] ||
  fail 'provider does not depend exactly on the two renamed AOSP parsers'
if nm -u "$temp_root/normal"/*.o | awk '{print $NF}' |
   grep -E '^_strto(d|f|ld)$' >/dev/null; then
  fail 'Darwin strto dependency escaped into provider archive'
fi

fixture="$temp_root/libbionic_float_fixture.so"
"$android_cc" -std=c17 -O2 -fno-builtin -fPIC -fno-stack-protector \
  -U_FORTIFY_SOURCE -D_FORTIFY_SOURCE=0 -Wall -Wextra -Werror \
  -shared -nostdlib -fuse-ld=lld -Wl,--build-id=none -Wl,--hash-style=sysv \
  -Wl,-z,now -Wl,-z,norelro -Wl,-z,max-page-size=16384 \
  -Wl,-soname,libbionic_float_fixture.so \
  -Wl,--version-script,"$script_dir/probes/exports.map" \
  "$script_dir/probes/fixture.c" -lc -o "$fixture"
check_hash "$fixture" "$FIXTURE_ELF_SHA256"
"$readelf" --dyn-syms --wide "$fixture" |
  awk '$7=="UND" && $8!="" {name=$8; sub(/@.*/,"",name); print name}' |
  sort -u > "$temp_root/fixture-imports"
printf '%s\n' __errno strtod strtod_l strtof strtof_l | sort > "$temp_root/expected-imports"
diff -u "$temp_root/expected-imports" "$temp_root/fixture-imports" ||
  fail 'Android ELF float import namespace drift'
[[ "$($elf_nm -D --defined-only "$fixture" | awk '$2=="T" {n++} END {print n+0}')" == 2 ]] ||
  fail 'fixture export count drift'

build_objects sanitized "$temp_root/sanitized"
sanitized_objects=("${built_objects[@]}")
"$host_cxx" -arch arm64 -isysroot "$sdk" -std=c++20 -O2 -Wall -Wextra \
  -Werror -fsanitize=address,undefined -fno-sanitize=shift -fno-omit-frame-pointer \
  -I"$script_dir/include" "$script_dir/probes/differential.cc" \
  "${sanitized_objects[@]}" -o "$temp_root/differential"
differential_output="$(ASAN_OPTIONS=halt_on_error=1 UBSAN_OPTIONS=halt_on_error=1 "$temp_root/differential")"
grep -F 'PASS AOSP-corpus=47x4x2 bits+end+errno rounding=4 host-fenv=preserved threads=8x1000 ASan+UBSan(no-shift)=clean' <<< "$differential_output" >/dev/null ||
  fail 'AOSP differential/sanitizer gate failed'

CARGO_TARGET_DIR="$temp_root/cargo-production" cargo build --quiet --lib \
  --manifest-path "$script_dir/Cargo.toml"
CARGO_TARGET_DIR="$temp_root/cargo-target" cargo run --quiet --features audit \
  --manifest-path "$script_dir/Cargo.toml" -- "$fixture"
CARGO_TARGET_DIR="$temp_root/cargo-target" cargo clippy --quiet --features audit \
  --manifest-path "$script_dir/Cargo.toml" -- -D warnings
cargo fmt --manifest-path "$script_dir/Cargo.toml" -- --check
[[ ! -d "$script_dir/target" ]] || fail 'module-local target directory forbidden'

build_dir="$project_root/_build/bionic-float-conversion-facade"
mkdir -p "$build_dir"
cp "$temp_root/libdarwin-art-bionic-float-conversion.a" "$build_dir/"
cp "$fixture" "$build_dir/"
printf '%s\n' "$differential_output"
echo 'bionic-float-conversion-facade: PASS demand=4 supported=2 rejected=2 system-extensions=2 AndroidELF=4+errno AOSP-gdtoa locale-ignored host-strto=0 long-double=binary128-rejected'
