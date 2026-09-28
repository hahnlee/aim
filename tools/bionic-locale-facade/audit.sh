#!/bin/bash
set -euo pipefail
export LC_ALL=C

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/../.." && pwd)"
source "$script_dir/sources.lock"

fail() { echo "bionic-locale-facade: $*" >&2; exit 3; }
missing() { echo "bionic-locale-facade: missing $*" >&2; exit 2; }
sha() { shasum -a 256 "$1" | awk '{print $1}'; }
check_hash() { [[ "$(sha "$1")" == "$2" ]] || fail "hash mismatch: $1"; }

master="$project_root/tools/bionic-libc-leaf-facade/imports/ndk-r28c-api35-arm64-libc.tsv"
errno_root="$project_root/tools/bionic-errno-tls"
icu_lock="$project_root/upstream/android16-icu-foundation.lock"
icu_root="$project_root/_aosp/external/icu-graphics"
icu_foundation="$project_root/_build/icu-foundation"
icu_common="$icu_foundation/libicuuc-common-darwin.a"
icu_stubdata="$icu_foundation/libicuuc-stubdata-darwin.a"
icu_init="$icu_foundation/libandroidicuinit-darwin.a"
icu_data="$icu_foundation/runtime/i18n/etc/icu/$ICU_DATA_FILE"
check_hash "$master" "$LIBC_IMPORT_MANIFEST_SHA256"
check_hash "$script_dir/upstream-sources.tsv" "$BIONIC_SOURCE_MANIFEST_SHA256"
check_hash "$script_dir/manifests/locale-cluster.tsv" "$LOCALE_CLUSTER_SHA256"
check_hash "$script_dir/manifests/imports.tsv" "$LOCALE_IMPORTS_SHA256"
check_hash "$script_dir/manifests/extension-boundary.tsv" "$LOCALE_EXTENSIONS_SHA256"
check_hash "$errno_root/sources.lock" "$BIONIC_ERRNO_LOCK_SHA256"
check_hash "$errno_root/src/errno_tls.c" "$BIONIC_ERRNO_SOURCE_SHA256"
check_hash "$errno_root/generated/darwin_to_android.inc" "$BIONIC_ERRNO_MAPPING_SHA256"
check_hash "$icu_lock" "$ICU_FOUNDATION_LOCK_SHA256"
check_hash "$icu_root/icu4c/source/common/unicode/uvernum.h" "$ICU_UVERNUM_SHA256"
check_hash "$icu_root/icu4c/source/common/unicode/uchar.h" "$ICU_UCHAR_SHA256"
check_hash "$icu_root/libandroidicuinit/include/androidicuinit/android_icu_init.h" "$ICU_INIT_HEADER_SHA256"
for archive in "$icu_common" "$icu_stubdata" "$icu_init"; do
  [[ -f "$archive" ]] || missing "$archive"
done
check_hash "$icu_data" "$ICU_DATA_SHA256"
[[ "$(stat -f %z "$icu_data")" == "$ICU_DATA_SIZE" ]] || fail 'ICU data size drift'
[[ "$(awk '$1=="#define" && $2=="U_ICU_VERSION" {gsub(/"/, "", $3); print $3}' "$icu_root/icu4c/source/common/unicode/uvernum.h")" == "$ICU_LIBRARY_VERSION" ]] || fail 'ICU header version drift'
check_hash "$script_dir/include/aim_bionic_locale.h" "$HEADER_SHA256"
check_hash "$script_dir/src/provider.cc" "$PROVIDER_SHA256"
check_hash "$script_dir/probes/fixture.c" "$FIXTURE_SHA256"
check_hash "$script_dir/probes/exports.map" "$EXPORTS_SHA256"
check_hash "$script_dir/probes/abi.c" "$ABI_SHA256"
check_hash "$script_dir/probes/stress.cc" "$STRESS_SHA256"
check_hash "$script_dir/probes/icu_differential.cc" "$ICU_DIFFERENTIAL_SHA256"

temp_root="$(mktemp -d "${TMPDIR:-/tmp}/bionic-locale-facade.XXXXXX")"
cleanup() {
  if [[ "$temp_root" == "${TMPDIR:-/tmp}"/bionic-locale-facade.* ]]; then
    find "$temp_root" -depth -delete
  fi
}
trap cleanup EXIT

awk -F '\t' 'NR>1 && $3=="C" && $4 ~ /locale/ {print $1 "\t" $3}' \
  "$master" | sort > "$temp_root/master-cluster"
tail -n +2 "$script_dir/manifests/locale-cluster.tsv" | cut -f1-2 | sort \
  > "$temp_root/locked-cluster"
diff -u "$temp_root/master-cluster" "$temp_root/locked-cluster" ||
  fail 'Class-C locale cluster drift'
[[ "$(wc -l < "$temp_root/master-cluster" | tr -d ' ')" == "$LOCALE_CLUSTER_COUNT" ]] ||
  fail 'locale cluster count drift'
[[ "$(awk -F '\t' 'NR>1 && $3=="supported"{n++}END{print n+0}' "$script_dir/manifests/locale-cluster.tsv")" == "$SUPPORTED_IMPORT_COUNT" ]] ||
  fail 'supported locale count drift'
[[ "$(awk -F '\t' 'NR>1 && $3=="unsupported"{n++}END{print n+0}' "$script_dir/manifests/locale-cluster.tsv")" == "$UNSUPPORTED_IMPORT_COUNT" ]] ||
  fail 'unsupported locale count drift'
awk -F '\t' 'NR>1 && $3=="supported"{print $1}' \
  "$script_dir/manifests/locale-cluster.tsv" | sort > "$temp_root/cluster-supported"
tail -n +2 "$script_dir/manifests/extension-boundary.tsv" | cut -f1 | sort \
  > "$temp_root/extension-supported"
cat "$temp_root/cluster-supported" "$temp_root/extension-supported" | sort \
  > "$temp_root/provider-expected"
tail -n +2 "$script_dir/manifests/imports.tsv" | cut -f1 | sort \
  > "$temp_root/provider-imports"
diff -u "$temp_root/provider-expected" "$temp_root/provider-imports" ||
  fail 'provider manifest is not the exact supported cluster plus extensions'
[[ "$(wc -l < "$temp_root/extension-supported" | tr -d ' ')" == "$LOCALE_EXTENSION_COUNT" ]] ||
  fail 'locale extension count drift'

source_root="$project_root/_aosp/bionic-locale-facade"
[[ "$(tail -n +2 "$script_dir/upstream-sources.tsv" | wc -l | tr -d ' ')" == "$BIONIC_SOURCE_COUNT" ]] ||
  fail 'upstream source count drift'
while IFS=$'\t' read -r relative size expected; do
  [[ "$relative" != "path" ]] || continue
  destination="$source_root/$relative"
  if [[ ! -f "$destination" ]]; then
    mkdir -p "$(dirname "$destination")"
    staged="$(mktemp "${TMPDIR:-/tmp}/bionic-locale-source.XXXXXX")"
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
  fail 'Git metadata forbidden in sparse locale source'

python3 - "$source_root" <<'PY'
import sys
from pathlib import Path
r=Path(sys.argv[1])
locale=(r/'libc/bionic/locale.cpp').read_text()
decode=(r/'libc/bionic/mbrtoc32.cpp').read_text()
encode=(r/'libc/bionic/c32rtomb.cpp').read_text()
wctype=(r/'libc/bionic/wctype.cpp').read_text()
strftime=(r/'libc/tzcode/strftime.c').read_text()
strcoll=(r/'libc/upstream-openbsd/lib/libc/string/strcoll.c').read_text()
strxfrm=(r/'libc/upstream-openbsd/lib/libc/string/strxfrm.c').read_text()
wcscoll=(r/'libc/upstream-openbsd/lib/libc/locale/wcscoll.c').read_text()
wcsxfrm=(r/'libc/upstream-openbsd/lib/libc/locale/wcsxfrm.c').read_text()
assert 'We only support two locales' in locale
for name in ('"C"', '"POSIX"', '"C.UTF-8"', '"en_US.UTF-8"'):
    assert name in locale
assert 'static bool __bionic_current_locale_is_utf8 = true' in locale
assert 'struct __locale_t' in locale and 'size_t mb_cur_max' in locale
assert 'LC_GLOBAL_LOCALE' in locale and 'get_current_locale_ptr' in locale
assert 'c32 < lower_bound' in decode and 'c32 >= 0xd800 && c32 <= 0xdfff' in decode
assert 'c32 > 0x10ffff' in decode and 'BIONIC_MULTIBYTE_RESULT_INCOMPLETE_SEQUENCE' in decode
assert '(c32 & ~0x1fffff) == 0' in encode
assert '__find_icu_symbol' in wctype
for property_name in ('UCHAR_ALPHABETIC', 'UCHAR_POSIX_BLANK',
                      'UCHAR_LOWERCASE', 'UCHAR_POSIX_PRINT',
                      'UCHAR_WHITE_SPACE', 'UCHAR_UPPERCASE',
                      'UCHAR_POSIX_XDIGIT'):
    assert property_name in wctype
for api_name in ('u_charType', 'u_isdigit', 'u_ispunct', 'u_tolower', 'u_toupper'):
    assert f'__find_icu_symbol("{api_name}")' in wctype
assert 'towlower_l' in wctype and 'towupper_l' in wctype
assert 'Just call strftime, as only the C locale is supported' in strftime
assert 'tzset();' in strftime
assert 'return (strcmp(s1, s2));' in strcoll
assert 'return (strlcpy(dst, src, n));' in strxfrm
assert 'return (wcscmp(s1, s2));' in wcscoll
assert 'return wcslcpy(dest, src, n);' in wcsxfrm
for name in ('iswalpha_l', 'iswblank_l', 'iswcntrl_l', 'iswdigit_l',
             'iswlower_l', 'iswprint_l', 'iswpunct_l', 'iswspace_l',
             'iswupper_l', 'iswxdigit_l'):
    assert f'int {name}(wint_t c, locale_t)' in wctype
assert 'return u_tolower ? u_tolower(wc)' in wctype
assert 'return u_toupper ? u_toupper(wc)' in wctype
print('bionic-locale-facade: upstream=PASS sources=21 supported=31 unsupported=tzcode1 wide-owner=Android-ICU76')
PY

ndk_root="${ANDROID_NDK_ROOT:-$HOME/Library/Android/sdk/ndk/$NDK_REVISION}"
toolchain="$ndk_root/toolchains/llvm/prebuilt/darwin-x86_64/bin"
android_cc="$toolchain/aarch64-linux-android${ANDROID_API}-clang"
readelf="$toolchain/llvm-readelf"
elf_nm="$toolchain/llvm-nm"
for input in "$ndk_root/source.properties" \
             "$ndk_root/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include/locale.h" \
             "$ndk_root/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include/wchar.h" \
             "$ndk_root/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include/wctype.h"; do
  [[ -f "$input" ]] || missing "$input"
done
check_hash "$ndk_root/source.properties" "$NDK_SOURCE_PROPERTIES_SHA256"
check_hash "$ndk_root/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include/locale.h" "$NDK_LOCALE_H_SHA256"
check_hash "$ndk_root/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include/wchar.h" "$NDK_WCHAR_H_SHA256"
check_hash "$ndk_root/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include/wctype.h" "$NDK_WCTYPE_H_SHA256"
"$android_cc" -std=c17 -Wall -Wextra -Werror -fsyntax-only "$script_dir/probes/abi.c"

sdk="$(xcrun --sdk macosx --show-sdk-path)"
host_cc="$(xcrun --find clang)"
host_cxx="$(xcrun --find clang++)"
host_flags=(-arch arm64 -isysroot "$sdk" -O2 -Wall -Wextra -Werror -fno-builtin)
"$host_cxx" "${host_flags[@]}" -std=c++20 -fvisibility=hidden \
  -DANDROID -I"$script_dir/include" \
  -I"$icu_root/android_icu4c/include" \
  -I"$icu_root/icu4c/source/common" \
  -I"$icu_root/libandroidicuinit/include" \
  -c "$script_dir/src/provider.cc" \
  -o "$temp_root/provider.o"
file "$temp_root/provider.o" | grep -F 'Mach-O 64-bit object arm64' >/dev/null ||
  fail 'provider is not Darwin arm64'
if nm -u "$temp_root/provider.o" | sed 's/^[[:space:]]*//' |
   grep -E '^_(setlocale|newlocale|freelocale|uselocale|localeconv|mbrtowc|wcrtomb|wcslen|wcscmp|isw|tow)' >/dev/null; then
  fail 'Darwin locale/multibyte implementation escaped into provider'
fi
if rg -n 'dlsym|dlopen|dyld|RTLD_|setlocale\(' "$script_dir/src/provider.cc" |
   grep -v 'aim_bionic_setlocale' >/dev/null; then
  fail 'host dynamic/global locale fallback entered provider'
fi
definitions="$(nm -gU "$temp_root/provider.o")"
undefined_icu="$(nm -u "$temp_root/provider.o" | awk '{print $NF}' | grep -E '^(_u_(charType|isdigit|ispunct|hasBinaryProperty|tolower|toupper|getVersion)_76|__Z16android_icu_initv|__Z25android_icu_is_registeredv)$' | sort -u)"
[[ "$(printf '%s\n' "$undefined_icu" | sed '/^$/d' | wc -l | tr -d ' ')" == 9 ]] ||
  fail 'provider ICU 76 static ABI dependency drift'
while IFS=$'\t' read -r symbol _; do
  [[ "$symbol" != "symbol" ]] || continue
  grep -F " _aim_bionic_$symbol" <<< "$definitions" >/dev/null ||
    fail "missing prefixed locale definition: $symbol"
done < "$script_dir/manifests/imports.tsv"
ar rcs "$temp_root/libaim-bionic-locale.a" "$temp_root/provider.o"

"$host_cc" "${host_flags[@]}" -std=c17 \
  -I"$errno_root/include" -I"$errno_root/generated" \
  -c "$errno_root/src/errno_tls.c" -o "$temp_root/errno-plain.o"

fixture="$temp_root/libbionic_locale_fixture.so"
"$android_cc" -std=c17 -O2 -fno-builtin -fPIC -fno-stack-protector \
  -U_FORTIFY_SOURCE -D_FORTIFY_SOURCE=0 -Wall -Wextra -Werror \
  -shared -nostdlib -fuse-ld=lld -Wl,--build-id=none -Wl,--hash-style=sysv \
  -Wl,-z,now -Wl,-z,norelro -Wl,-z,max-page-size=16384 \
  -Wl,-soname,libbionic_locale_fixture.so \
  -Wl,--version-script,"$script_dir/probes/exports.map" \
  "$script_dir/probes/fixture.c" -lc -o "$fixture"
check_hash "$fixture" "$FIXTURE_ELF_SHA256"
file "$fixture" | grep -F 'ELF 64-bit LSB shared object, ARM aarch64' >/dev/null ||
  fail 'fixture is not Android arm64 ELF'
"$readelf" --dyn-syms --wide "$fixture" |
  awk '$7=="UND" && $8!="" {name=$8; sub(/@.*/,"",name); print name}' |
  sort -u > "$temp_root/fixture-imports"
tail -n +2 "$script_dir/manifests/imports.tsv" | cut -f1 > "$temp_root/expected-imports"
printf '%s\n' __errno >> "$temp_root/expected-imports"
sort -u "$temp_root/expected-imports" -o "$temp_root/expected-imports"
diff -u "$temp_root/expected-imports" "$temp_root/fixture-imports" ||
  fail 'Android ELF locale import namespace drift'
[[ "$($elf_nm -D --defined-only "$fixture" | awk '$2=="T"{n++}END{print n+0}')" == 5 ]] ||
  fail 'fixture export count drift'

"$host_cc" "${host_flags[@]}" -std=c17 -fsanitize=address,undefined \
  -I"$errno_root/include" -I"$errno_root/generated" \
  -c "$errno_root/src/errno_tls.c" -o "$temp_root/errno.o"
"$host_cxx" "${host_flags[@]}" -std=c++20 -fsanitize=address,undefined \
  -DANDROID -I"$script_dir/include" \
  -I"$icu_root/android_icu4c/include" \
  -I"$icu_root/icu4c/source/common" \
  -I"$icu_root/libandroidicuinit/include" \
  -c "$script_dir/src/provider.cc" \
  -o "$temp_root/provider-asan.o"
"$host_cxx" "${host_flags[@]}" -std=c++20 -fsanitize=address,undefined \
  -I"$script_dir/include" "$script_dir/probes/stress.cc" \
  "$temp_root/provider-asan.o" "$temp_root/errno.o" \
  -Wl,-force_load,"$icu_init" "$icu_common" "$icu_stubdata" \
  -o "$temp_root/stress"

runtime_root="$temp_root/runtime"
mkdir -p "$runtime_root/data" "$runtime_root/tzdata" "$runtime_root/i18n/etc/icu"
ln -s "$icu_data" "$runtime_root/i18n/etc/icu/$ICU_DATA_FILE"
run_with_icu() {
  ANDROID_DATA="$runtime_root/data" \
  ANDROID_TZDATA_ROOT="$runtime_root/tzdata" \
  ANDROID_I18N_ROOT="$runtime_root/i18n" "$@"
}

stress_output="$(run_with_icu "$temp_root/stress")"
grep -F 'threads=8 rounds=100 handles=0 UTF-8-invalid+incomplete ICU76-wide=concurrent state=thread-local ASan+UBSan=clean' <<< "$stress_output" >/dev/null ||
  fail 'locale sanitizer stress failed'

"$host_cxx" "${host_flags[@]}" -std=c++20 -DANDROID \
  -I"$script_dir/include" -I"$icu_root/android_icu4c/include" \
  -I"$icu_root/icu4c/source/common" \
  -I"$icu_root/libandroidicuinit/include" \
  "$script_dir/probes/icu_differential.cc" "$temp_root/provider.o" \
  "$temp_root/errno-plain.o" -Wl,-force_load,"$icu_init" \
  "$icu_common" "$icu_stubdata" -o "$temp_root/icu-differential"
differential_output="$(run_with_icu "$temp_root/icu-differential")"
grep -F 'PASS ICU=76.1 codepoints=34 locales=null+global+invalid Bionic-_l=locale-ignored' <<< "$differential_output" >/dev/null ||
  fail 'ICU differential gate failed'

for executable in "$temp_root/stress" "$temp_root/icu-differential"; do
  if otool -L "$executable" | grep -E '(/opt/homebrew|/usr/local|libicu(uc|i18n))' >/dev/null; then
    fail "host or dynamic ICU dependency escaped: $executable"
  fi
done

run_with_icu env CARGO_TARGET_DIR="$temp_root/cargo-target" cargo run --quiet \
  --manifest-path "$script_dir/Cargo.toml" -- "$fixture"
if otool -L "$temp_root/cargo-target/debug/bionic-locale-facade" |
   grep -E '(/opt/homebrew|/usr/local|libicu(uc|i18n))' >/dev/null; then
  fail 'Rust Android ELF gate acquired host or dynamic ICU'
fi
CARGO_TARGET_DIR="$temp_root/cargo-target" cargo clippy --quiet \
  --manifest-path "$script_dir/Cargo.toml" -- -D warnings
cargo fmt --manifest-path "$script_dir/Cargo.toml" -- --check

build_dir="$project_root/_build/bionic-locale-facade"
mkdir -p "$build_dir"
cp "$temp_root/libaim-bionic-locale.a" "$build_dir/"
cp "$fixture" "$build_dir/"
printf '%s\n' "$stress_output"
printf '%s\n' "$differential_output"
echo 'bionic-locale-facade: PASS cluster=32 supported=31 unsupported=1 AndroidELF=31+errno ICU=76.1 static-data host-global-locale=0'
