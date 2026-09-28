#!/bin/bash
set -euo pipefail
export LC_ALL=C

dir="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$dir/../.." && pwd)"
# shellcheck disable=SC1091
source "$dir/sources.lock"

fail() { echo "bionic-wide-stdio-facade: $*" >&2; exit 3; }
missing() { echo "bionic-wide-stdio-facade: missing $*" >&2; exit 2; }
sha() { shasum -a 256 "$1" | awk '{print $1}'; }
check() { [[ "$(sha "$1")" == "$2" ]] || fail "hash: $1"; }
clean() {
  [[ -z "$(find "$dir" -type d -name target -print -quit)" ]] || fail 'local target exists'
  git -C "$root" diff --check -- tools/bionic-wide-stdio-facade || fail 'tracked diff check'
  git -C "$root" diff --cached --check -- tools/bionic-wide-stdio-facade || fail 'staged diff check'
  while IFS= read -r -d '' file; do
    set +e
    whitespace="$(git -C "$root" diff --no-index --check /dev/null "$file" 2>&1)"
    status=$?
    set -e
    [[ -z "$whitespace" ]] || fail "untracked whitespace: $file: $whitespace"
    [[ $status -le 1 ]] || fail "could not diff-check untracked file: $file"
  done < <(git -C "$root" ls-files --others --exclude-standard -z -- tools/bionic-wide-stdio-facade)
}

clean
tmp="$(mktemp -d "${TMPDIR:-/tmp}/bionic-wide-stdio.XXXXXX")"
cleanup() {
  if [[ "$tmp" == "${TMPDIR:-/tmp}"/bionic-wide-stdio.* ]]; then
    find "$tmp" -depth -delete
  fi
}
trap cleanup EXIT

master="$root/tools/bionic-libc-leaf-facade/imports/ndk-r28c-api35-arm64-libc.tsv"
errno_root="$root/tools/bionic-errno-tls"
locale_root="$root/tools/bionic-locale-facade"
locale_archive="$root/_build/bionic-locale-facade/libaim-bionic-locale.a"
icu_root="$root/_aosp/external/icu-graphics"
icu_build="$root/_build/icu-foundation"
icu_common="$icu_build/libicuuc-common-darwin.a"
icu_stubdata="$icu_build/libicuuc-stubdata-darwin.a"
icu_init="$icu_build/libandroidicuinit-darwin.a"

check "$master" "$LIBC_IMPORT_MANIFEST_SHA256"
check "$errno_root/sources.lock" "$BIONIC_ERRNO_LOCK_SHA256"
check "$errno_root/src/errno_tls.c" "$BIONIC_ERRNO_SOURCE_SHA256"
check "$errno_root/generated/darwin_to_android.inc" "$BIONIC_ERRNO_MAPPING_SHA256"
check "$locale_root/sources.lock" "$BIONIC_LOCALE_LOCK_SHA256"
check "$locale_root/include/aim_bionic_locale.h" "$BIONIC_LOCALE_HEADER_SHA256"
[[ -f "$locale_archive" ]] || missing "$locale_archive"
check "$root/tools/bionic-stdio-facade/include/aim_bionic_stdio.h" "$BIONIC_STDIO_HEADER_SHA256"
check "$root/upstream/android16-icu-foundation.lock" "$ICU_FOUNDATION_LOCK_SHA256"
for archive in "$icu_common" "$icu_stubdata" "$icu_init"; do
  [[ -f "$archive" ]] || missing "$archive"
done
check "$icu_root/icu4c/source/common/unicode/uvernum.h" "$ICU_UVERNUM_SHA256"
[[ "$(awk '$1=="#define"&&$2=="U_ICU_VERSION"{gsub(/"/,"",$3);print $3}' "$icu_root/icu4c/source/common/unicode/uvernum.h")" == "76.1" ]] ||
  fail 'ICU version is not pinned 76.1'

check "$dir/upstream-sources.tsv" "$UPSTREAM_SOURCES_SHA256"
check "$dir/manifests/demand.tsv" "$DEMAND_SHA256"
check "$dir/manifests/imports.tsv" "$IMPORTS_SHA256"
check "$dir/include/aim_bionic_wide_stdio.h" "$HEADER_SHA256"
check "$dir/src/provider.cc" "$PROVIDER_SHA256"
check "$dir/src/shims.c" "$SHIMS_SHA256"
check "$dir/src/main.rs" "$MAIN_SHA256"
check "$dir/probes/backend.h" "$BACKEND_SHA256"
check "$dir/probes/elf_backend.cc" "$ELF_BACKEND_SHA256"
check "$dir/probes/stress.cc" "$STRESS_SHA256"
check "$dir/probes/fixture.c" "$FIXTURE_SHA256"
check "$dir/probes/exports.map" "$EXPORTS_SHA256"
check "$dir/probes/abi.c" "$ABI_SHA256"
check "$dir/build.rs" "$BUILD_RS_SHA256"
check "$dir/Cargo.toml" "$CARGO_TOML_SHA256"
check "$dir/README.md" "$README_SHA256"
check "$dir/audit.sh" "$AUDIT_SHA256"

awk -F '\t' 'NR>1&&($1=="fputwc"||$1=="getwc"||$1=="ungetwc"){print $1"\t"$2"\t"$3"\t"$4}' \
  "$master" | sort > "$tmp/master.imports"
tail -n +2 "$dir/manifests/demand.tsv" | sort > "$tmp/locked.imports"
diff -u "$tmp/master.imports" "$tmp/locked.imports" ||
  fail 'pinned libc++ wide stdio demand drift'
[[ "$(wc -l < "$tmp/master.imports" | tr -d ' ')" == 3 ]] || fail 'wide stdio demand count drift'
tail -n +2 "$dir/manifests/demand.tsv" | cut -f1 | sort > "$tmp/demand.symbols"
tail -n +2 "$dir/manifests/imports.tsv" | cut -f1 | sort > "$tmp/provider.symbols"
diff -u "$tmp/demand.symbols" "$tmp/provider.symbols" || fail 'demand/provider symbol drift'

source_root="$root/_aosp/bionic-wide-stdio-facade"
[[ "$(tail -n +2 "$dir/upstream-sources.tsv" | wc -l | tr -d ' ')" == 5 ]] ||
  fail 'upstream source count drift'
while IFS=$'\t' read -r relative size expected; do
  [[ "$relative" != path ]] || continue
  destination="$source_root/$relative"
  if [[ ! -f "$destination" ]]; then
    mkdir -p "$(dirname "$destination")"
    staged="$(mktemp "${TMPDIR:-/tmp}/bionic-wide-stdio-source.XXXXXX")"
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

python3 - "$source_root" <<'PY'
import sys
from pathlib import Path
r = Path(sys.argv[1])
get = (r/'libc/upstream-openbsd/lib/libc/stdio/fgetwc.c').read_text()
put = (r/'libc/upstream-openbsd/lib/libc/stdio/fputwc.c').read_text()
unget = (r/'libc/upstream-openbsd/lib/libc/stdio/ungetwc.c').read_text()
local = (r/'libc/stdio/local.h').read_text()
stdio = (r/'libc/stdio/stdio.cpp').read_text()
assert '_SET_ORIENTATION(fp, 1);' in get
assert 'if (wcio->wcio_ungetwc_inbuf)' in get
assert 'int ch = __sgetc(fp);' in get
assert 'if (ch == EOF) {\n\t\t\treturn WEOF;' in get
assert 'size = mbrtowc(&wc, &c, 1, st);' in get
assert 'fp->_flags |= __SERR;' in get
assert '} while (size == (size_t)-2);' in get
assert 'wcio->wcio_ungetwc_inbuf = 0;' in put
assert put.index('wcio->wcio_ungetwc_inbuf = 0;') < put.index('size = wcrtomb(buf, wc, st);')
assert 'if (__sfvwrite(fp, &uio))' in put
assert 'if (wc == WEOF)' in unget
assert 'wcio->wcio_ungetwc_inbuf >= WCIO_UNGETWC_BUFSIZE' in unget
assert '__sclearerr(fp);' in unget
assert '#define WCIO_UNGETWC_BUFSIZE 1' in local
assert 'mbstate_t wcio_mbstate_in;' in local and 'mbstate_t wcio_mbstate_out;' in local
assert 'wint_t getwc(FILE* fp) {' in stdio and 'CHECK_FP(fp);\n  return fgetwc(fp);' in stdio
print('bionic-wide-stdio-facade: upstream=PASS getwc-alias fgetwc/fputwc/ungetwc mbstate=2 pushback=1 partial-EOF=plain-WEOF')
PY

ndk="${ANDROID_NDK_ROOT:-$HOME/Library/Android/sdk/ndk/$NDK_REVISION}"
tc="$ndk/toolchains/llvm/prebuilt/darwin-x86_64/bin"
android_cc="$tc/aarch64-linux-android${ANDROID_API}-clang"
readelf="$tc/llvm-readelf"
elf_nm="$tc/llvm-nm"
ndk_stdio="$ndk/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include/stdio.h"
ndk_wchar="$ndk/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include/wchar.h"
libc="$ndk/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/lib/aarch64-linux-android/$ANDROID_API/libc.so"
for input in "$ndk/source.properties" "$ndk_stdio" "$ndk_wchar" "$libc"; do
  [[ -f "$input" ]] || missing "$input"
done
check "$ndk/source.properties" "$NDK_SOURCE_PROPERTIES_SHA256"
check "$ndk_stdio" "$NDK_STDIO_H_SHA256"
check "$ndk_wchar" "$NDK_WCHAR_H_SHA256"
check "$libc" "$NDK_API35_ARM64_LIBC_SHA256"
for symbol in fputwc getwc ungetwc; do
  "$readelf" --dyn-syms --wide "$libc" |
    awk -v wanted="$symbol@@LIBC" '$4=="FUNC"&&$5=="GLOBAL"&&$8==wanted{found=1}END{exit !found}' ||
    fail "API-35 libc export: $symbol@@LIBC"
done
"$android_cc" -std=c17 -Wall -Wextra -Werror -Wpedantic \
  -I"$dir/include" -I"$root/tools/bionic-stdio-facade/include" \
  -fsyntax-only "$dir/probes/abi.c"

sdk="$(xcrun --sdk macosx --show-sdk-path)"
host_cc="$(xcrun --find clang)"
host_cxx="$(xcrun --find clang++)"
host_flags=(-arch arm64 -isysroot "$sdk" -O1 -g -Wall -Wextra -Werror -fno-builtin -fvisibility=hidden)
"$host_cxx" "${host_flags[@]}" -std=c++20 -I"$dir/include" -I"$locale_root/include" \
  -c "$dir/src/provider.cc" -o "$tmp/provider.o"
"$host_cc" "${host_flags[@]}" -std=c17 -I"$dir/include" \
  -c "$dir/src/shims.c" -o "$tmp/shims.o"

definitions="$(nm -gU "$tmp/provider.o" "$tmp/shims.o")"
while IFS=$'\t' read -r symbol _; do
  [[ "$symbol" != symbol ]] || continue
  grep -F " _aim_bionic_$symbol" <<< "$definitions" >/dev/null ||
    fail "missing prefixed definition: $symbol"
done < "$dir/manifests/imports.tsv"
for forbidden in fgetwc putwc getwchar putwchar ungetwc_unlocked; do
  ! grep -F " _aim_bionic_$forbidden" <<< "$definitions" >/dev/null ||
    fail "unsupported wide stdio definition: $forbidden"
done
! nm -u "$tmp/provider.o" "$tmp/shims.o" | awk '{print $NF}' |
  grep -E '^_(fputwc|getwc|ungetwc|fgetwc|putwc|mbrtowc|wcrtomb|isw|tow|wctype)' >/dev/null ||
  fail 'Darwin wide stdio/character dependency escaped'
! rg -n 'dlsym|dlopen|dyld|RTLD_|reinterpret_cast<.*(FILE|wchar)|<wchar\.h>|<wctype\.h>' \
  "$dir/src" "$dir/include" >/dev/null || fail 'host/dynamic wide implementation escaped'

fixture="$tmp/libbionic_wide_stdio_fixture.so"
"$android_cc" -std=c17 -O2 -fno-builtin -fPIC -fno-stack-protector \
  -U_FORTIFY_SOURCE -D_FORTIFY_SOURCE=0 -Wall -Wextra -Werror -Wpedantic \
  -shared -nostdlib -fuse-ld=lld -Wl,--build-id=none -Wl,--hash-style=sysv \
  -Wl,-z,now -Wl,-z,norelro -Wl,-z,max-page-size=16384 \
  -Wl,-soname,libbionic_wide_stdio_fixture.so \
  -Wl,--version-script,"$dir/probes/exports.map" \
  "$dir/probes/fixture.c" -lc -o "$fixture"
check "$fixture" "$FIXTURE_ELF_SHA256"
file "$fixture" | grep -F 'ELF 64-bit LSB shared object, ARM aarch64' >/dev/null ||
  fail 'fixture is not Android arm64 ELF'
"$readelf" --dyn-syms --wide "$fixture" |
  awk '$7=="UND"&&$8!=""{print $8}' | sort -u > "$tmp/fixture.imports"
cat > "$tmp/fixture.expected" <<'EOF'
__errno@LIBC
fputwc@LIBC
getwc@LIBC
ungetwc@LIBC
EOF
diff -u "$tmp/fixture.expected" "$tmp/fixture.imports" ||
  fail 'Android ELF exact import namespace drift'
[[ "$("$elf_nm" -D --defined-only "$fixture" | awk '$2=="T"{n++}END{print n+0}')" == 1 ]] ||
  fail 'fixture export count drift'

san=(-fsanitize=address,undefined -fno-omit-frame-pointer)
"$host_cxx" "${host_flags[@]}" "${san[@]}" -std=c++20 \
  -I"$dir/include" -I"$locale_root/include" \
  -c "$dir/src/provider.cc" -o "$tmp/provider-san.o"
"$host_cc" "${host_flags[@]}" "${san[@]}" -std=c17 -I"$dir/include" \
  -c "$dir/src/shims.c" -o "$tmp/shims-san.o"
"$host_cc" "${host_flags[@]}" "${san[@]}" -std=c17 \
  -I"$errno_root/include" -I"$errno_root/generated" \
  -c "$errno_root/src/errno_tls.c" -o "$tmp/errno-san.o"
"$host_cxx" "${host_flags[@]}" "${san[@]}" -std=c++20 \
  -I"$dir/include" -I"$locale_root/include" -I"$errno_root/include" \
  "$dir/probes/stress.cc" "$tmp/provider-san.o" "$tmp/shims-san.o" \
  "$tmp/errno-san.o" "$locale_archive" -Wl,-force_load,"$icu_init" \
  "$icu_common" "$icu_stubdata" -o "$tmp/stress"
stress_output="$(ASAN_OPTIONS=halt_on_error=1 UBSAN_OPTIONS=halt_on_error=1 "$tmp/stress")"
grep -F 'PASS UTF-8+surrogate-asymmetry+partial-EOF+pushback threads=8 per-stream-parallel close/reset-race=serialized token-reuse=clean host-errno=preserved ASan+UBSan=clean' \
  <<< "$stress_output" >/dev/null || fail 'wide stdio sanitizer stress failed'

CARGO_TARGET_DIR="$tmp/normal" cargo run --quiet --manifest-path "$dir/Cargo.toml" -- "$fixture"
CARGO_TARGET_DIR="$tmp/normal" cargo clippy --quiet --all-targets \
  --manifest-path "$dir/Cargo.toml" -- -D warnings
ASAN_OPTIONS=halt_on_error=1 BIONIC_WIDE_STDIO_C_SANITIZER=address \
  CARGO_TARGET_DIR="$tmp/asan" cargo run --quiet --manifest-path "$dir/Cargo.toml" -- "$fixture"
UBSAN_OPTIONS=halt_on_error=1 BIONIC_WIDE_STDIO_C_SANITIZER=undefined \
  CARGO_TARGET_DIR="$tmp/ubsan" cargo run --quiet --manifest-path "$dir/Cargo.toml" -- "$fixture"
cargo fmt --manifest-path "$dir/Cargo.toml" -- --check
for executable in "$tmp/stress" "$tmp/normal/debug/bionic-wide-stdio-facade"; do
  ! otool -L "$executable" | grep -E '(/opt/homebrew|/usr/local|libicu(uc|i18n))' >/dev/null ||
    fail "host/dynamic ICU dependency escaped: $executable"
done
clean

build="$root/_build/bionic-wide-stdio-facade"
mkdir -p "$build"
ar rcs "$build/libaim-bionic-wide-stdio.a" "$tmp/provider.o" "$tmp/shims.o"
cp "$fixture" "$build/"
printf '%s\n' "$stress_output"
echo 'bionic-wide-stdio-facade: PASS demand=3 AndroidELF=3+errno FILE=opaque152 Android-wchar32 UTF-8+EOF+pushback threads=8 close/reset+reuse C-ASan C-UBSan ICU=76.1 target-clean'
