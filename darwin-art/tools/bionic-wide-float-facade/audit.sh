#!/bin/bash
set -euo pipefail
export LC_ALL=C

dir="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$dir/../.." && pwd)"
# shellcheck disable=SC1091
source "$dir/sources.lock"

fail() { echo "bionic-wide-float-facade: $*" >&2; exit 3; }
missing() { echo "bionic-wide-float-facade: missing $*" >&2; exit 2; }
sha() { shasum -a 256 "$1" | awk '{print $1}'; }
check() { [[ "$(sha "$1")" == "$2" ]] || fail "hash: $1"; }
clean() {
  [[ -z "$(find "$dir" -type d -name target -print -quit)" ]] || fail 'local target exists'
  git -C "$root" diff --check -- tools/bionic-wide-float-facade || fail 'tracked diff check'
  git -C "$root" diff --cached --check -- tools/bionic-wide-float-facade || fail 'staged diff check'
  while IFS= read -r -d '' file; do
    set +e
    whitespace="$(git -C "$root" diff --no-index --check /dev/null "$file" 2>&1)"
    status=$?
    set -e
    [[ -z "$whitespace" ]] || fail "untracked whitespace: $file: $whitespace"
    [[ $status -le 1 ]] || fail "could not diff-check untracked file: $file"
  done < <(git -C "$root" ls-files --others --exclude-standard -z -- tools/bionic-wide-float-facade)
}

clean
tmp="$(mktemp -d "${TMPDIR:-/tmp}/bionic-wide-float.XXXXXX")"
cleanup() {
  if [[ "$tmp" == "${TMPDIR:-/tmp}"/bionic-wide-float.* ]]; then
    find "$tmp" -depth -delete
  fi
}
trap cleanup EXIT

master="$root/tools/bionic-libc-leaf-facade/imports/ndk-r28c-api35-arm64-libc.tsv"
float_root="$root/tools/bionic-float-conversion-facade"
float_archive="$root/_build/bionic-float-conversion-facade/libdarwin-art-bionic-float-conversion.a"
allocator_root="$root/tools/bionic-libc-allocator-facade"
errno_root="$root/tools/bionic-errno-tls"
icu_lock="$root/upstream/android16-icu-foundation.lock"
icu_root="$root/_aosp/external/icu-graphics"
icu_build="$root/_build/icu-foundation"
icu_common="$icu_build/libicuuc-common-darwin.a"
icu_stubdata="$icu_build/libicuuc-stubdata-darwin.a"
icu_init="$icu_build/libandroidicuinit-darwin.a"
icu_data="$icu_build/runtime/i18n/etc/icu/$ICU_DATA_FILE"

check "$master" "$LIBC_IMPORT_MANIFEST_SHA256"
check "$float_root/sources.lock" "$FLOAT_FACADE_LOCK_SHA256"
check "$allocator_root/src/allocator.c" "$ALLOCATOR_SOURCE_SHA256"
check "$allocator_root/src/allocator_options.c" "$ALLOCATOR_OPTIONS_SOURCE_SHA256"
check "$allocator_root/include/darwin_art_bionic_allocator.h" "$ALLOCATOR_HEADER_SHA256"
check "$icu_lock" "$ICU_FOUNDATION_LOCK_SHA256"
for archive in "$icu_common" "$icu_stubdata" "$icu_init"; do
  [[ -f "$archive" ]] || missing "$archive"
done
check "$icu_data" "$ICU_DATA_SHA256"
[[ "$(stat -f %z "$icu_data")" == "$ICU_DATA_SIZE" ]] || fail 'ICU data size drift'
check "$dir/upstream-sources.tsv" "$UPSTREAM_SOURCES_SHA256"
check "$dir/manifests/demand.tsv" "$DEMAND_SHA256"
check "$dir/manifests/imports.tsv" "$IMPORTS_SHA256"
check "$dir/include/darwin_art_bionic_wide_float.h" "$HEADER_SHA256"
check "$dir/src/provider.cc" "$PROVIDER_SHA256"
check "$dir/src/main.rs" "$MAIN_SHA256"
check "$dir/probes/fixture.c" "$FIXTURE_SHA256"
check "$dir/probes/abi.c" "$ABI_SHA256"
check "$dir/probes/differential.cc" "$DIFFERENTIAL_SHA256"
check "$dir/probes/exports.map" "$EXPORTS_SHA256"
check "$dir/build.rs" "$BUILD_RS_SHA256"
check "$dir/Cargo.toml" "$CARGO_TOML_SHA256"
check "$dir/README.md" "$README_SHA256"

awk -F '\t' 'NR>1 && ($1=="wcstod" || $1=="wcstof" || $1=="wcstold") {
  print $1 "\t" $2 "\t" $3
}' "$master" | sort > "$tmp/master-demand"
tail -n +2 "$dir/manifests/demand.tsv" | cut -f1-3 | sort > "$tmp/locked-demand"
diff -u "$tmp/master-demand" "$tmp/locked-demand" ||
  fail 'pinned libc++ wide float demand drift'
[[ "$(wc -l < "$tmp/master-demand" | tr -d ' ')" == 3 ]] || fail 'demand count drift'
awk -F '\t' 'NR>1&&$4=="supported"{print $1}' "$dir/manifests/demand.tsv" |
  sort > "$tmp/supported"
tail -n +2 "$dir/manifests/imports.tsv" | cut -f1 | sort > "$tmp/imports"
diff -u "$tmp/supported" "$tmp/imports" || fail 'supported/import manifest drift'
[[ "$(awk -F '\t' 'NR>1&&$4=="supported"{n++}END{print n+0}' "$dir/manifests/demand.tsv")" == 2 ]] ||
  fail 'supported count drift'
[[ "$(awk -F '\t' 'NR>1&&$4=="rejected"{n++}END{print n+0}' "$dir/manifests/demand.tsv")" == 1 ]] ||
  fail 'rejected count drift'

source_root="$root/_aosp/bionic-wide-float-facade"
[[ "$(tail -n +2 "$dir/upstream-sources.tsv" | wc -l | tr -d ' ')" == "$BIONIC_SOURCE_COUNT" ]] ||
  fail 'upstream source count drift'
while IFS=$'\t' read -r relative size expected; do
  [[ "$relative" != path ]] || continue
  destination="$source_root/$relative"
  if [[ ! -f "$destination" ]]; then
    mkdir -p "$(dirname "$destination")"
    staged="$(mktemp "${TMPDIR:-/tmp}/bionic-wide-float-source.XXXXXX")"
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
r=Path(sys.argv[1])
bp=(r/'libc/Android.bp').read_text()
s=(r/'libc/bionic/wcstod.cpp').read_text()
assert '"bionic/wcstod.cpp"' in bp
assert 'template <typename float_type>' in s
assert 'while (iswspace(*str))' in s
assert 'wcsspn(str, L"-+0123456789.xXeEpP()nNaAiIfFtTyY")' in s
assert 'char* ascii_str = new char[max_len + 1]' in s
assert 'actual_len = parsefloat' in s
assert 'ascii_end != ascii_str + actual_len' in s
assert 'actual_len == 0' in s and 'original_str' in s
assert 'return wcstod<float>(s, end, strtof);' in s
assert 'return wcstod<double>(s, end, strtod);' in s
assert 'return wcstod<long double>(s, end, strtold);' in s
print('bionic-wide-float-facade: upstream=PASS wcstod.cpp allowed-ASCII+parsefloat+endptr wrappers=3')
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
for symbol in wcstod wcstof wcstold; do
  "$readelf" --dyn-syms --wide "$libc" |
    awk -v wanted="$symbol@@LIBC" '$4=="FUNC"&&$5=="GLOBAL"&&$8==wanted{found=1}END{exit !found}' ||
    fail "API-35 libc export: $symbol@@LIBC"
done
"$android_cc" -std=c17 -Wall -Wextra -Werror -Wpedantic -fsyntax-only "$dir/probes/abi.c"

# Reuse only independently accepted provider/allocator modules.
bash "$float_root/audit.sh" >/dev/null
bash "$allocator_root/audit.sh" >/dev/null
ar -t "$float_archive" > "$tmp/float-archive-members"
[[ "$(wc -l < "$tmp/float-archive-members" | tr -d ' ')" == "$FLOAT_ARCHIVE_MEMBER_COUNT" ]] ||
  fail 'float provider archive member count drift'
[[ "$(sha "$tmp/float-archive-members")" == "$FLOAT_ARCHIVE_MEMBERS_SHA256" ]] ||
  fail 'float provider archive member manifest drift'

sdk="$(xcrun --sdk macosx --show-sdk-path)"
host_cc="$(xcrun --find clang)"
host_cxx="$(xcrun --find clang++)"
host_flags=(-arch arm64 -isysroot "$sdk" -O2 -Wall -Wextra -Werror -fno-builtin -fvisibility=hidden)
provider_includes=(-I"$dir/include" -I"$float_root/include" -I"$allocator_root/include" \
  -I"$errno_root/include" -I"$icu_root/android_icu4c/include" \
  -I"$icu_root/icu4c/source/common" -I"$icu_root/libandroidicuinit/include")
"$host_cxx" "${host_flags[@]}" -std=c++20 -DANDROID "${provider_includes[@]}" \
  -c "$dir/src/provider.cc" -o "$tmp/provider.o"
"$host_cc" "${host_flags[@]}" -std=c17 -I"$allocator_root/include" \
  -c "$allocator_root/src/allocator.c" -o "$tmp/allocator.o"
"$host_cc" "${host_flags[@]}" -std=c17 -I"$allocator_root/include" \
  -c "$allocator_root/src/allocator_options.c" -o "$tmp/allocator_options.o"
file "$tmp/provider.o" | grep -F 'Mach-O 64-bit object arm64' >/dev/null ||
  fail 'provider is not Darwin arm64'
definitions="$(nm -gU "$tmp/provider.o")"
for symbol in wcstod wcstof wide_float_resolve wide_float_capability; do
  grep -F " _darwin_art_bionic_$symbol" <<< "$definitions" >/dev/null ||
    fail "missing prefixed definition: $symbol"
done
for forbidden in wcstold wcstod_l wcstof_l; do
  ! grep -F " _darwin_art_bionic_$forbidden" <<< "$definitions" >/dev/null ||
    fail "unsupported definition escaped: $forbidden"
done
if nm -u "$tmp/provider.o" | awk '{print $NF}' |
   grep -E '^_(wcsto|wcsspn|iswspace|iswctype|strtod|strtof|strtold)$' >/dev/null; then
  fail 'Darwin wchar/wctype/numeric implementation escaped into provider'
fi
if rg -n 'dlsym|dlopen|dyld|RTLD_|std::wc|wcsspn\(|iswspace\(' "$dir/src/provider.cc" >/dev/null; then
  fail 'host wide/dynamic fallback entered provider'
fi
for required in _darwin_art_bionic_malloc_result _darwin_art_bionic_free \
                _darwin_art_bionic_strtod _darwin_art_bionic_strtof \
                _darwin_art_bionic_errno_store _u_hasBinaryProperty_76; do
  nm -u "$tmp/provider.o" | awk '{print $NF}' | grep -Fx "$required" >/dev/null ||
    fail "missing provider dependency: $required"
done
ar rcs "$tmp/libdarwin-art-bionic-wide-float.a" "$tmp/provider.o"
[[ "$(ar -t "$tmp/libdarwin-art-bionic-wide-float.a" | grep -vc '^__.SYMDEF')" == 1 ]] ||
  fail 'product archive must contain only the wide-float owner'
file "$tmp/libdarwin-art-bionic-wide-float.a" | grep -F 'current ar archive' >/dev/null ||
  fail 'product output is not an archive'
product_definitions="$(nm -gU "$tmp/libdarwin-art-bionic-wide-float.a" | awk '{print $NF}')"
for foreign_owner in darwin_art_bionic_malloc darwin_art_bionic_free \
                     darwin_art_bionic_strtod darwin_art_bionic_strtof \
                     android_icu_init u_hasBinaryProperty_76; do
  ! grep -Fx "_$foreign_owner" <<< "$product_definitions" >/dev/null ||
    fail "foreign allocator/gdtoa/ICU owner embedded: $foreign_owner"
done

fixture="$tmp/libbionic_wide_float_fixture.so"
"$android_cc" -std=c17 -O2 -fno-builtin -fPIC -fno-stack-protector \
  -U_FORTIFY_SOURCE -D_FORTIFY_SOURCE=0 -Wall -Wextra -Werror -Wpedantic \
  -shared -nostdlib -fuse-ld=lld -Wl,--build-id=none -Wl,--hash-style=sysv \
  -Wl,-z,now -Wl,-z,norelro -Wl,-z,max-page-size=16384 \
  -Wl,-soname,libbionic_wide_float_fixture.so \
  -Wl,--version-script,"$dir/probes/exports.map" \
  "$dir/probes/fixture.c" -lc -o "$fixture"
check "$fixture" "$FIXTURE_ELF_SHA256"
file "$fixture" | grep -F 'ELF 64-bit LSB shared object, ARM aarch64' >/dev/null ||
  fail 'fixture is not Android arm64 ELF'
"$readelf" --dyn-syms --wide "$fixture" |
  awk '$7=="UND"&&$8!=""{print $8}' | sort -u > "$tmp/fixture.imports"
cat > "$tmp/fixture.expected" <<'EOF'
__errno@LIBC
wcstod@LIBC
wcstof@LIBC
EOF
diff -u "$tmp/fixture.expected" "$tmp/fixture.imports" ||
  fail 'Android ELF exact import namespace drift'
[[ "$("$elf_nm" -D --defined-only "$fixture" | awk '$2=="T"{n++}END{print n+0}')" == 2 ]] ||
  fail 'fixture export count drift'

san=(-O1 -g -fsanitize=address,undefined -fno-omit-frame-pointer)
"$host_cxx" "${host_flags[@]}" "${san[@]}" -std=c++20 -DANDROID \
  "${provider_includes[@]}" -c "$dir/src/provider.cc" -o "$tmp/provider-san.o"
"$host_cc" "${host_flags[@]}" "${san[@]}" -std=c17 -I"$allocator_root/include" \
  -c "$allocator_root/src/allocator.c" -o "$tmp/allocator-san.o"
"$host_cc" "${host_flags[@]}" "${san[@]}" -std=c17 -I"$allocator_root/include" \
  -c "$allocator_root/src/allocator_options.c" -o "$tmp/allocator-options-san.o"
"$host_cxx" "${host_flags[@]}" "${san[@]}" -std=c++20 \
  -I"$dir/include" -I"$icu_root/android_icu4c/include" \
  -I"$icu_root/icu4c/source/common" "$dir/probes/differential.cc" \
  "$tmp/provider-san.o" "$tmp/allocator-san.o" \
  "$tmp/allocator-options-san.o" \
  -Wl,-force_load,"$float_archive" -Wl,-force_load,"$icu_init" \
  "$icu_common" "$icu_stubdata" -o "$tmp/differential"

runtime_root="$tmp/runtime"
mkdir -p "$runtime_root/data" "$runtime_root/tzdata" "$runtime_root/i18n/etc/icu"
ln -s "$icu_data" "$runtime_root/i18n/etc/icu/$ICU_DATA_FILE"
run_with_icu() {
  ANDROID_DATA="$runtime_root/data" ANDROID_TZDATA_ROOT="$runtime_root/tzdata" \
    ANDROID_I18N_ROOT="$runtime_root/i18n" "$@"
}
differential_output="$(ASAN_OPTIONS=halt_on_error=1 UBSAN_OPTIONS=halt_on_error=1 \
  run_with_icu "$tmp/differential")"
grep -F 'PASS cases=58x4x2 Unicode-White_Space+nan+hex+overflow bits+end+errno threads=8x1000 host-errno+fenv=preserved ASan+UBSan(no-shift)=clean' \
  <<< "$differential_output" >/dev/null || fail 'differential/sanitizer gate failed'

run_with_icu env CARGO_TARGET_DIR="$tmp/cargo-normal" cargo run --quiet \
  --manifest-path "$dir/Cargo.toml" -- "$fixture"
run_with_icu env ASAN_OPTIONS=halt_on_error=1 BIONIC_WIDE_FLOAT_C_SANITIZER=address \
  CARGO_TARGET_DIR="$tmp/cargo-asan" cargo run --quiet \
  --manifest-path "$dir/Cargo.toml" -- "$fixture"
run_with_icu env UBSAN_OPTIONS=halt_on_error=1 BIONIC_WIDE_FLOAT_C_SANITIZER=undefined \
  CARGO_TARGET_DIR="$tmp/cargo-ubsan" cargo run --quiet \
  --manifest-path "$dir/Cargo.toml" -- "$fixture"
CARGO_TARGET_DIR="$tmp/cargo-clippy" cargo clippy --quiet --all-targets \
  --manifest-path "$dir/Cargo.toml" -- -D warnings
cargo fmt --manifest-path "$dir/Cargo.toml" -- --check
clean

build_dir="$root/_build/bionic-wide-float-facade"
mkdir -p "$build_dir"
cp "$tmp/libdarwin-art-bionic-wide-float.a" "$build_dir/"
cp "$fixture" "$build_dir/"
printf '%s\n' "$differential_output"
echo 'bionic-wide-float-facade: PASS demand=3 supported=2 rejected=wcstold-binary128 AndroidELF=2+errno ICU76 AOSP-gdtoa allocator+errno threads=8x1000 C-ASan C-UBSan closed=libc.so@LIBC'
