#!/bin/bash
set -euo pipefail
export LC_ALL=C

dir="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$dir/../.." && pwd)"
# shellcheck disable=SC1091
source "$dir/sources.lock"

fail() { echo "bionic-numeric-facade: $*" >&2; exit 3; }
missing() { echo "bionic-numeric-facade: missing $*" >&2; exit 2; }
sha() { shasum -a 256 "$1" | awk '{print $1}'; }
check() { [[ "$(sha "$1")" == "$2" ]] || fail "hash: $1"; }
clean() {
  [[ -z "$(find "$dir" -type d -name target -print -quit)" ]] || fail 'local target exists'
  git -C "$root" diff --check -- tools/bionic-numeric-facade || fail 'tracked diff check'
  git -C "$root" diff --cached --check -- tools/bionic-numeric-facade || fail 'staged diff check'
  while IFS= read -r -d '' file; do
    set +e
    whitespace="$(git -C "$root" diff --no-index --check /dev/null "$file" 2>&1)"
    status=$?
    set -e
    [[ -z "$whitespace" ]] || fail "untracked whitespace: $file: $whitespace"
    [[ $status -le 1 ]] || fail "could not diff-check untracked file: $file"
  done < <(git -C "$root" ls-files --others --exclude-standard -z -- tools/bionic-numeric-facade)
}

clean
tmp="$(mktemp -d "${TMPDIR:-/tmp}/bionic-numeric.XXXXXX")"
master="$root/tools/bionic-libc-leaf-facade/imports/ndk-r28c-api35-arm64-libc.tsv"
errno_root="$root/tools/bionic-errno-tls"
check "$master" "$LIBC_IMPORT_MANIFEST_SHA256"
check "$errno_root/sources.lock" "$BIONIC_ERRNO_LOCK_SHA256"
check "$errno_root/src/errno_tls.c" "$BIONIC_ERRNO_SOURCE_SHA256"
check "$errno_root/generated/darwin_to_android.inc" "$BIONIC_ERRNO_MAPPING_SHA256"
check "$dir/upstream-sources.tsv" "$UPSTREAM_SOURCES_SHA256"
check "$dir/manifests/imports.tsv" "$IMPORTS_SHA256"
check "$dir/include/darwin_art_bionic_numeric.h" "$HEADER_SHA256"
check "$dir/src/provider.c" "$PROVIDER_SHA256"
check "$dir/src/main.rs" "$MAIN_SHA256"
check "$dir/probes/fixture.c" "$FIXTURE_SHA256"
check "$dir/probes/abi.c" "$ABI_SHA256"
check "$dir/probes/differential.cc" "$DIFFERENTIAL_SHA256"
check "$dir/probes/exports.map" "$EXPORTS_SHA256"
check "$dir/build.rs" "$BUILD_RS_SHA256"
check "$dir/Cargo.toml" "$CARGO_TOML_SHA256"
check "$dir/README.md" "$README_SHA256"
check "$dir/audit.sh" "$AUDIT_SHA256"

tail -n +2 "$dir/manifests/imports.tsv" | cut -f1 | sort > "$tmp/provider-demand"
trap 'find "$tmp" -depth -delete' EXIT
cat > "$tmp/expected-demand" <<'EOF'
strtoimax
strtol
strtoll
strtoll_l
strtoul
strtoull
strtoull_l
EOF
diff -u "$tmp/expected-demand" "$tmp/provider-demand" || fail 'provider manifest drift'
for symbol in strtol strtoll strtoll_l strtoul strtoull strtoull_l; do
  awk -F '\t' -v wanted="$symbol" '$1==wanted && $2=="FUNC" && $3=="B"{found=1}END{exit !found}' \
    "$master" || fail "pinned libc++ demand: $symbol"
done
for absent in strtol_l strtoul_l; do
  ! awk -F '\t' -v wanted="$absent" '$1==wanted{found=1}END{exit !found}' "$master" ||
    fail "unexpected pinned libc++ demand: $absent"
done
[[ "$(tail -n +2 "$dir/manifests/imports.tsv" | wc -l | tr -d ' ')" == 7 ]] ||
  fail 'supported import count'

source_root="$root/_aosp/bionic-numeric-facade"
while IFS=$'\t' read -r relative size expected; do
  [[ "$relative" != path ]] || continue
  destination="$source_root/$relative"
  if [[ ! -f "$destination" ]]; then
    mkdir -p "$(dirname "$destination")"
    staged="$(mktemp "${TMPDIR:-/tmp}/bionic-numeric-source.XXXXXX")"
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

python3 - "$source_root" <<'PY'
import sys
from pathlib import Path
r = Path(sys.argv[1])
integer = (r/'libc/bionic/strtol.cpp').read_text()
locale = (r/'libc/bionic/stdlib_l.cpp').read_text()
assert 'base < 0 || base == 1 || base > 36' in integer
assert "(*p == 'x' || *p == 'X') && isxdigit(p[1])" in integer
assert "(*p == 'b' || *p == 'B') && isdigit(p[1])" in integer
assert '__builtin_mul_overflow' in integer
assert 'errno = ERANGE' in integer and 'errno = EINVAL' in integer
assert 'return neg ? -acc : acc;' in integer
for name in ('strtol', 'strtoll', 'strtoimax', 'strtoul', 'strtoull'):
    assert f'{name}(' in integer
for name in ('strtol_l', 'strtoll_l', 'strtoul_l', 'strtoull_l'):
    body = locale[locale.index(name + '('):]
    assert f'return {name[:-2]}(s, end_ptr, base);' in body[:240]
print('bionic-numeric-facade: upstream=PASS integer-overflow+prefix locale-wrapper=opaque')
PY

ndk="${ANDROID_NDK_ROOT:-$HOME/Library/Android/sdk/ndk/$NDK_REVISION}"
tc="$ndk/toolchains/llvm/prebuilt/darwin-x86_64/bin"
cc="$tc/aarch64-linux-android${ANDROID_API}-clang"
readelf="$tc/llvm-readelf"
libc="$ndk/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/lib/aarch64-linux-android/$ANDROID_API/libc.so"
for input in "$ndk/source.properties" \
             "$ndk/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include/stdlib.h" \
             "$libc"; do
  [[ -f "$input" ]] || missing "$input"
done
check "$ndk/source.properties" "$NDK_SOURCE_PROPERTIES_SHA256"
check "$ndk/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include/stdlib.h" "$NDK_STDLIB_H_SHA256"
check "$libc" "$NDK_API35_ARM64_LIBC_SHA256"
for symbol in strtol strtoll strtoimax strtoll_l strtoul strtoull strtoull_l; do
  "$readelf" --dyn-syms --wide "$libc" |
    awk -v wanted="$symbol@@LIBC" '$4=="FUNC"&&$5=="GLOBAL"&&$8==wanted{found=1}END{exit !found}' ||
    fail "API-35 libc export: $symbol@@LIBC"
done
"$cc" -std=c17 -Wall -Wextra -Werror -Wpedantic -fsyntax-only "$dir/probes/abi.c"

sdk="$(xcrun --sdk macosx --show-sdk-path)"
host_cc="$(xcrun --find clang)"
host_cxx="$(xcrun --find clang++)"
host_flags=(-arch arm64 -isysroot "$sdk" -O1 -g -Wall -Wextra -Werror -Wpedantic -fno-builtin)
"$host_cc" "${host_flags[@]}" -std=c17 -I"$dir/include" \
  -c "$dir/src/provider.c" -o "$tmp/provider.o"
nm -u "$tmp/provider.o" | sed 's/^[[:space:]]*//' | sort > "$tmp/provider.undefined"
cat > "$tmp/provider.expected" <<'EOF'
___error
_darwin_art_bionic_errno_store
EOF
diff -u "$tmp/provider.expected" "$tmp/provider.undefined" ||
  fail 'provider escaped into host numeric/locale implementation'
definitions="$(nm -gU "$tmp/provider.o")"
while IFS=$'\t' read -r symbol _; do
  [[ "$symbol" != symbol ]] || continue
  grep -F " _darwin_art_bionic_$symbol" <<< "$definitions" >/dev/null ||
    fail "missing prefixed definition: $symbol"
done < "$dir/manifests/imports.tsv"
for forbidden in strtod strtof strtold strtol_l strtoul_l wcstol; do
  ! grep -F " _darwin_art_bionic_$forbidden" <<< "$definitions" >/dev/null ||
    fail "unsupported definition: $forbidden"
done
! rg -n 'dlsym|dlopen|dyld|RTLD_' "$dir/src/provider.c" >/dev/null ||
  fail 'dynamic/global fallback in provider'

fixture="$tmp/libbionic_numeric_fixture.so"
"$cc" -std=c17 -O2 -fno-builtin -fPIC -fno-stack-protector \
  -U_FORTIFY_SOURCE -D_FORTIFY_SOURCE=0 -Wall -Wextra -Werror -Wpedantic \
  -shared -nostdlib -fuse-ld=lld -Wl,--build-id=none -Wl,--hash-style=sysv \
  -Wl,-z,now -Wl,-z,norelro -Wl,-z,max-page-size=16384 \
  -Wl,-soname,libbionic_numeric_fixture.so \
  -Wl,--version-script,"$dir/probes/exports.map" \
  "$dir/probes/fixture.c" -lc -o "$fixture"
file "$fixture" | grep -F 'ELF 64-bit LSB shared object, ARM aarch64' >/dev/null ||
  fail 'fixture is not Android arm64 ELF'
"$readelf" --dyn-syms --wide "$fixture" |
  awk '$7=="UND"&&$8!=""{print $8}' | sort -u > "$tmp/fixture.imports"
cat > "$tmp/fixture.expected" <<'EOF'
__errno@LIBC
strtoimax@LIBC
strtol@LIBC
strtoll@LIBC
strtoll_l@LIBC
strtoul@LIBC
strtoull@LIBC
strtoull_l@LIBC
EOF
diff -u "$tmp/fixture.expected" "$tmp/fixture.imports" ||
  fail 'Android ELF exact import namespace drift'

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
UBSAN_OPTIONS=halt_on_error=1 "$tmp/differential"

CARGO_TARGET_DIR="$tmp/normal" cargo run --quiet --manifest-path "$dir/Cargo.toml" -- "$fixture"
CARGO_TARGET_DIR="$tmp/normal" cargo clippy --quiet --all-targets \
  --manifest-path "$dir/Cargo.toml" -- -D warnings
BIONIC_NUMERIC_C_SANITIZER=address CARGO_TARGET_DIR="$tmp/asan" \
  cargo run --quiet --manifest-path "$dir/Cargo.toml" -- "$fixture"
UBSAN_OPTIONS=halt_on_error=1 BIONIC_NUMERIC_C_SANITIZER=undefined \
  CARGO_TARGET_DIR="$tmp/ubsan" cargo run --quiet --manifest-path "$dir/Cargo.toml" -- "$fixture"
cargo fmt --manifest-path "$dir/Cargo.toml" -- --check
clean
echo 'bionic-numeric-facade: PASS libc++=7 AndroidELF=8@LIBC AOSP-differential base=0,2..36 host-strto=0 threads=8x1000 C-ASan C-UBSan target-clean'
