#!/bin/bash
set -euo pipefail
export LC_ALL=C

dir="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$dir/../.." && pwd)"
# shellcheck disable=SC1091
source "$dir/sources.lock"

fail() { echo "bionic-wide-integer-facade: $*" >&2; exit 3; }
missing() { echo "bionic-wide-integer-facade: missing $*" >&2; exit 2; }
sha() { shasum -a 256 "$1" | awk '{print $1}'; }
check() { [[ "$(sha "$1")" == "$2" ]] || fail "hash: $1"; }
clean() {
  [[ -z "$(find "$dir" -type d -name target -print -quit)" ]] || fail 'local target exists'
  git -C "$root" diff --check -- tools/bionic-wide-integer-facade || fail 'tracked diff check'
  git -C "$root" diff --cached --check -- tools/bionic-wide-integer-facade || fail 'staged diff check'
  while IFS= read -r -d '' file; do
    set +e
    whitespace="$(git -C "$root" diff --no-index --check /dev/null "$file" 2>&1)"
    status=$?
    set -e
    [[ -z "$whitespace" ]] || fail "untracked whitespace: $file: $whitespace"
    [[ $status -le 1 ]] || fail "could not diff-check untracked file: $file"
  done < <(git -C "$root" ls-files --others --exclude-standard -z -- tools/bionic-wide-integer-facade)
}

clean
tmp="$(mktemp -d "${TMPDIR:-/tmp}/bionic-wide-integer.XXXXXX")"
cleanup() {
  if [[ "$tmp" == "${TMPDIR:-/tmp}"/bionic-wide-integer.* ]]; then
    find "$tmp" -depth -delete
  fi
}
trap cleanup EXIT

master="$root/tools/bionic-libc-leaf-facade/imports/ndk-r28c-api35-arm64-libc.tsv"
errno_root="$root/tools/bionic-errno-tls"
check "$master" "$LIBC_IMPORT_MANIFEST_SHA256"
check "$errno_root/sources.lock" "$BIONIC_ERRNO_LOCK_SHA256"
check "$errno_root/src/errno_tls.c" "$BIONIC_ERRNO_SOURCE_SHA256"
check "$errno_root/generated/darwin_to_android.inc" "$BIONIC_ERRNO_MAPPING_SHA256"
check "$dir/upstream-sources.tsv" "$UPSTREAM_SOURCES_SHA256"
check "$dir/manifests/demand.tsv" "$DEMAND_SHA256"
check "$dir/manifests/imports.tsv" "$IMPORTS_SHA256"
check "$dir/include/aim_bionic_wide_integer.h" "$HEADER_SHA256"
check "$dir/src/provider.c" "$PROVIDER_SHA256"
check "$dir/src/main.rs" "$MAIN_SHA256"
check "$dir/probes/fixture.c" "$FIXTURE_SHA256"
check "$dir/probes/abi.c" "$ABI_SHA256"
check "$dir/probes/differential.cc" "$DIFFERENTIAL_SHA256"
check "$dir/probes/exports.map" "$EXPORTS_SHA256"
check "$dir/build.rs" "$BUILD_RS_SHA256"
check "$dir/Cargo.toml" "$CARGO_TOML_SHA256"
check "$dir/README.md" "$README_SHA256"

awk -F '\t' 'NR>1 && ($1=="wcstod" || $1=="wcstof" || $1=="wcstol" ||
  $1=="wcstold" || $1=="wcstoll" || $1=="wcstoul" || $1=="wcstoull") {
  print $1 "\t" $2 "\t" $3
}' "$master" | sort > "$tmp/master-demand"
tail -n +2 "$dir/manifests/demand.tsv" | cut -f1-3 | sort > "$tmp/locked-demand"
diff -u "$tmp/master-demand" "$tmp/locked-demand" ||
  fail 'pinned libc++ wide conversion demand drift'
[[ "$(wc -l < "$tmp/master-demand" | tr -d ' ')" == 7 ]] ||
  fail 'wide conversion demand count drift'
awk -F '\t' 'NR>1 && $4=="supported" {print $1}' "$dir/manifests/demand.tsv" |
  sort > "$tmp/supported"
tail -n +2 "$dir/manifests/imports.tsv" | cut -f1 | sort > "$tmp/imports"
diff -u "$tmp/supported" "$tmp/imports" || fail 'supported/import manifest drift'
[[ "$(wc -l < "$tmp/imports" | tr -d ' ')" == 4 ]] || fail 'support count drift'
[[ "$(awk -F '\t' 'NR>1&&$4=="rejected"{n++}END{print n+0}' "$dir/manifests/demand.tsv")" == 3 ]] ||
  fail 'rejection count drift'
for absent in wcstol_l wcstoll_l wcstoul_l wcstoull_l; do
  ! awk -F '\t' -v wanted="$absent" '$1==wanted{found=1}END{exit !found}' "$master" ||
    fail "unexpected pinned libc++ demand: $absent"
done

source_root="$root/_aosp/bionic-wide-integer-facade"
[[ "$(tail -n +2 "$dir/upstream-sources.tsv" | wc -l | tr -d ' ')" == "$BIONIC_SOURCE_COUNT" ]] ||
  fail 'upstream source count drift'
while IFS=$'\t' read -r relative size expected; do
  [[ "$relative" != path ]] || continue
  destination="$source_root/$relative"
  if [[ ! -f "$destination" ]]; then
    mkdir -p "$(dirname "$destination")"
    staged="$(mktemp "${TMPDIR:-/tmp}/bionic-wide-source.XXXXXX")"
    curl -fsSL "https://android.googlesource.com/$BIONIC_PROJECT/+/$BIONIC_REVISION/$relative?format=TEXT" |
      base64 -D > "$staged"
    [[ "$(stat -f %z "$staged")" == "$size" && "$(sha "$staged")" == "$expected" ]] ||
      fail "download provenance: $relative"
    mv "$staged" "$destination"
  fi
  [[ "$(stat -f %z "$destination")" == "$size" && "$(sha "$destination")" == "$expected" ]] ||
    fail "sparse source drift: $relative"
done < "$dir/upstream-sources.tsv"
printf '%s\n' "$BIONIC_REVISION" > "$source_root/.source-revision"
[[ -z "$(find "$source_root" \( -name .git -o -name .gitmodules \) -print -quit)" ]] ||
  fail 'Git metadata forbidden in sparse source'

python3 - "$source_root/libc/bionic/strtol.cpp" <<'PY'
import sys
from pathlib import Path
s = Path(sys.argv[1]).read_text()
assert 'template <typename T, T Min, T Max, typename CharT>' in s
assert 'base < 0 || base == 1 || base > 36' in s
assert "(*p == 'x' || *p == 'X') && isxdigit(p[1])" in s
assert "(*p == 'b' || *p == 'B') && isdigit(p[1])" in s
assert '__builtin_mul_overflow' in s
assert 'errno = ERANGE' in s and 'errno = EINVAL' in s
assert 'return neg ? -acc : acc;' in s
for name in ('wcstol', 'wcstoll', 'wcstoul', 'wcstoull'):
    assert f'{name}(const wchar_t* s, wchar_t** end, int base)' in s
print('bionic-wide-integer-facade: upstream=PASS AOSP-generic-char+wchar base+prefix+overflow+unsigned-negation')
PY

ndk="${ANDROID_NDK_ROOT:-$HOME/Library/Android/sdk/ndk/$NDK_REVISION}"
tc="$ndk/toolchains/llvm/prebuilt/darwin-x86_64/bin"
android_cc="$tc/aarch64-linux-android${ANDROID_API}-clang"
readelf="$tc/llvm-readelf"
elf_nm="$tc/llvm-nm"
ndk_wchar="$ndk/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include/wchar.h"
libc="$ndk/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/lib/aarch64-linux-android/$ANDROID_API/libc.so"
for input in "$ndk/source.properties" "$ndk_wchar" "$libc"; do
  [[ -f "$input" ]] || missing "$input"
done
check "$ndk/source.properties" "$NDK_SOURCE_PROPERTIES_SHA256"
check "$ndk_wchar" "$NDK_WCHAR_H_SHA256"
check "$libc" "$NDK_API35_ARM64_LIBC_SHA256"
for symbol in wcstol wcstoll wcstoul wcstoull; do
  "$readelf" --dyn-syms --wide "$libc" |
    awk -v wanted="$symbol@@LIBC" '$4=="FUNC"&&$5=="GLOBAL"&&$8==wanted{found=1}END{exit !found}' ||
    fail "API-35 libc export: $symbol@@LIBC"
done
"$android_cc" -std=c17 -Wall -Wextra -Werror -Wpedantic -fsyntax-only "$dir/probes/abi.c"

sdk="$(xcrun --sdk macosx --show-sdk-path)"
host_cc="$(xcrun --find clang)"
host_cxx="$(xcrun --find clang++)"
host_flags=(-arch arm64 -isysroot "$sdk" -O1 -g -Wall -Wextra -Werror -fno-builtin)
"$host_cc" "${host_flags[@]}" -std=c17 -I"$dir/include" \
  -c "$dir/src/provider.c" -o "$tmp/provider.o"
"$host_cc" "${host_flags[@]}" -std=c17 \
  -I"$errno_root/include" -I"$errno_root/generated" \
  -c "$errno_root/src/errno_tls.c" -o "$tmp/errno.o"
nm -u "$tmp/provider.o" | sed 's/^[[:space:]]*//' | sort > "$tmp/provider.undefined"
cat > "$tmp/provider.expected" <<'EOF'
___error
_aim_bionic_errno_store
_fegetenv
_fegetround
_feraiseexcept
_fesetenv
_fesetround
_fetestexcept
_strcmp
EOF
diff -u "$tmp/provider.expected" "$tmp/provider.undefined" ||
  fail 'provider escaped into host wchar/ctype/numeric implementation'
definitions="$(nm -gU "$tmp/provider.o")"
while IFS=$'\t' read -r symbol _; do
  [[ "$symbol" != symbol ]] || continue
  grep -F " _aim_bionic_$symbol" <<< "$definitions" >/dev/null ||
    fail "missing prefixed definition: $symbol"
done < "$dir/manifests/imports.tsv"
for forbidden in wcstod wcstof wcstold wcstol_l wcstoll_l wcstoul_l wcstoull_l strtol; do
  ! grep -F " _aim_bionic_$forbidden" <<< "$definitions" >/dev/null ||
    fail "unsupported definition: $forbidden"
done
! nm -u "$tmp/provider.o" | awk '{print $NF}' |
  grep -E '^_(wcsto|isw|tow|wctype|iswctype|__maskrune|strto)' >/dev/null ||
  fail 'Darwin wchar/wctype/numeric dependency escaped'
! rg -n 'dlsym|dlopen|dyld|RTLD_' "$dir/src/provider.c" >/dev/null ||
  fail 'dynamic/global fallback in provider'

fixture="$tmp/libbionic_wide_integer_fixture.so"
"$android_cc" -std=c17 -O2 -fno-builtin -fPIC -fno-stack-protector \
  -U_FORTIFY_SOURCE -D_FORTIFY_SOURCE=0 -Wall -Wextra -Werror -Wpedantic \
  -shared -nostdlib -fuse-ld=lld -Wl,--build-id=none -Wl,--hash-style=sysv \
  -Wl,-z,now -Wl,-z,norelro -Wl,-z,max-page-size=16384 \
  -Wl,-soname,libbionic_wide_integer_fixture.so \
  -Wl,--version-script,"$dir/probes/exports.map" \
  "$dir/probes/fixture.c" -lc -o "$fixture"
check "$fixture" "$FIXTURE_ELF_SHA256"
file "$fixture" | grep -F 'ELF 64-bit LSB shared object, ARM aarch64' >/dev/null ||
  fail 'fixture is not Android arm64 ELF'
"$readelf" --dyn-syms --wide "$fixture" |
  awk '$7=="UND"&&$8!=""{print $8}' | sort -u > "$tmp/fixture.imports"
cat > "$tmp/fixture.expected" <<'EOF'
__errno@LIBC
wcstol@LIBC
wcstoll@LIBC
wcstoul@LIBC
wcstoull@LIBC
EOF
diff -u "$tmp/fixture.expected" "$tmp/fixture.imports" ||
  fail 'Android ELF exact import namespace drift'
[[ "$("$elf_nm" -D --defined-only "$fixture" | awk '$2=="T"{n++}END{print n+0}')" == 2 ]] ||
  fail 'fixture export count drift'

san=(-fsanitize=address,undefined -fno-omit-frame-pointer)
"$host_cc" "${host_flags[@]}" "${san[@]}" -std=c17 -I"$dir/include" \
  -c "$dir/src/provider.c" -o "$tmp/provider-san.o"
"$host_cc" "${host_flags[@]}" "${san[@]}" -std=c17 \
  -I"$errno_root/include" -I"$errno_root/generated" \
  -c "$errno_root/src/errno_tls.c" -o "$tmp/errno-san.o"
renames=(atoi atol atoll strtoimax strtol strtoll strtoul strtoull strtoumax \
         wcstoimax wcstol wcstoll wcstoul wcstoull wcstoumax)
rename_flags=()
for name in "${renames[@]}"; do rename_flags+=("-D$name=aosp_$name"); done
"$host_cxx" "${host_flags[@]}" "${san[@]}" -std=c++20 "${rename_flags[@]}" \
  -c "$source_root/libc/bionic/strtol.cpp" -o "$tmp/aosp-strtol.o"
"$host_cxx" "${host_flags[@]}" "${san[@]}" -std=c++20 -I"$dir/include" \
  "$dir/probes/differential.cc" "$tmp/provider-san.o" "$tmp/errno-san.o" \
  "$tmp/aosp-strtol.o" -o "$tmp/differential"
differential_output="$(ASAN_OPTIONS=halt_on_error=1 UBSAN_OPTIONS=halt_on_error=1 "$tmp/differential")"
grep -F 'PASS AOSP-byte/wchar32=5084 bases=-2..38 edges=8 threads=8x1000 host-errno+fenv=preserved ASan+UBSan=clean' \
  <<< "$differential_output" >/dev/null || fail 'differential/sanitizer gate failed'

CARGO_TARGET_DIR="$tmp/normal" cargo run --quiet --manifest-path "$dir/Cargo.toml" -- "$fixture"
CARGO_TARGET_DIR="$tmp/normal" cargo clippy --quiet --all-targets \
  --manifest-path "$dir/Cargo.toml" -- -D warnings
ASAN_OPTIONS=halt_on_error=1 BIONIC_WIDE_INTEGER_C_SANITIZER=address \
  CARGO_TARGET_DIR="$tmp/asan" cargo run --quiet --manifest-path "$dir/Cargo.toml" -- "$fixture"
UBSAN_OPTIONS=halt_on_error=1 BIONIC_WIDE_INTEGER_C_SANITIZER=undefined \
  CARGO_TARGET_DIR="$tmp/ubsan" cargo run --quiet --manifest-path "$dir/Cargo.toml" -- "$fixture"
cargo fmt --manifest-path "$dir/Cargo.toml" -- --check
clean

build_dir="$root/_build/bionic-wide-integer-facade"
mkdir -p "$build_dir"
ar rcs "$build_dir/libaim-bionic-wide-integer.a" "$tmp/provider.o" "$tmp/errno.o"
cp "$fixture" "$build_dir/"
printf '%s\n' "$differential_output"
echo 'bionic-wide-integer-facade: PASS demand=7 supported=4 rejected=3 AndroidELF=4+errno AOSP-wchar32 host-wcsto=0 threads=8x1000 C-ASan C-UBSan target-clean'
