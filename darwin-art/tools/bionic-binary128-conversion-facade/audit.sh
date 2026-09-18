#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd)"
repo_root="$(cd "$script_dir/../.." && pwd)"
source_root="$repo_root/_aosp/bionic-binary128-conversion-facade"
build_root="$repo_root/_build/bionic-binary128-conversion-facade"
BIONIC_REVISION=361ba86734fb2821a6adcfdf775db8abd04e0de0
NDK_REVISION=28.2.13676358
ANDROID_API=35

fail() { echo "bionic-binary128-conversion-facade: $*" >&2; exit 1; }
sha() { shasum -a 256 "$1" | awk '{print $1}'; }
check_hash() { [[ "$(sha "$1")" == "$2" ]] || fail "hash drift: $1"; }
bash -n "$script_dir/audit.sh"

# shellcheck disable=SC1091
source "$script_dir/sources.lock"
check_hash "$script_dir/audit.sh" "$AUDIT_SHA256"
[[ "$BIONIC_REVISION" == 361ba86734fb2821a6adcfdf775db8abd04e0de0 ]] ||
  fail 'Bionic revision lock drift'
[[ "$NDK_REVISION" == 28.2.13676358 && "$ANDROID_API" == 35 ]] ||
  fail 'NDK lock drift'
check_hash "$script_dir/upstream-sources.tsv" "$BIONIC_UPSTREAM_SOURCES_SHA256"
check_hash "$script_dir/manifests/demand.tsv" "$DEMAND_SHA256"
check_hash "$script_dir/manifests/imports.tsv" "$IMPORTS_SHA256"
check_hash "$script_dir/include/darwin_art_bionic_binary128_conversion.h" "$HEADER_SHA256"
check_hash "$script_dir/src/provider.cc" "$PROVIDER_SHA256"
check_hash "$script_dir/probes/host_state.cc" "$HOST_STATE_HELPER_SHA256"
check_hash "$script_dir/src/entry.S" "$ENTRY_SHA256"
check_hash "$script_dir/src/main.rs" "$MAIN_SHA256"
check_hash "$script_dir/probes/fixture.c" "$FIXTURE_SHA256"
check_hash "$script_dir/probes/abi.c" "$ABI_SHA256"
check_hash "$script_dir/probes/differential.cc" "$DIFFERENTIAL_SHA256"
check_hash "$script_dir/probes/exports.map" "$EXPORTS_SHA256"
check_hash "$script_dir/build.rs" "$BUILD_RS_SHA256"
check_hash "$script_dir/Cargo.toml" "$CARGO_TOML_SHA256"
check_hash "$script_dir/README.md" "$README_SHA256"

while IFS=$'\t' read -r relative size digest; do
  [[ "$relative" != path ]] || continue
  destination="$source_root/$relative"
  if [[ ! -f "$destination" || "$(stat -f %z "$destination")" != "$size" ||
        "$(sha "$destination")" != "$digest" ]]; then
    mkdir -p "$(dirname "$destination")"
    encoded="$(mktemp)"
    trap 'rm -f "$encoded"' EXIT
    curl -fsSL "https://android.googlesource.com/platform/bionic/+/$BIONIC_REVISION/$relative?format=TEXT" -o "$encoded"
    base64 -D < "$encoded" > "$destination"
    rm -f "$encoded"
    trap - EXIT
  fi
  [[ "$(stat -f %z "$destination")" == "$size" && "$(sha "$destination")" == "$digest" ]] ||
    fail "pinned source drift: $relative"
done < "$script_dir/upstream-sources.tsv"
printf '%s\n' "$BIONIC_REVISION" > "$source_root/.source-revision"
[[ -z "$(find "$source_root" \( -name .git -o -name .gitmodules \) -print -quit)" ]] ||
  fail 'Git metadata forbidden in sparse source'

python3 - "$source_root" "$script_dir" <<'PY'
import sys
from pathlib import Path
r=Path(sys.argv[1]); s=Path(sys.argv[2])
bp=(r/'libc/Android.bp').read_text()
st=(r/'libc/bionic/strtold.cpp').read_text()
sl=(r/'libc/bionic/stdlib_l.cpp').read_text()
q=(r/'libc/upstream-openbsd/lib/libc/gdtoa/strtorQ.c').read_text()
assert 'srcs: ["upstream-openbsd/lib/libc/gdtoa/strtorQ.c"]' in bp
assert '__strtorQ(s, end_ptr, FLT_ROUNDS, &result)' in st
assert 'long double strtold_l' in sl and 'return strtold(s, end_ptr);' in sl
assert 'strtodg(s, sp, fpi, &exp, bits)' in q and '113, 1-16383-113+1' in q
provider=(s/'src/provider.cc').read_text()
entry=(s/'src/entry.S').read_text()
assert 'long double' not in provider
assert 'ldr q0, [sp, #16]' in entry
assert '(void)locale' in provider
print('source-contract=PASS Android16-strtorQ+Bionic-locale-wrapper')
PY

check_hash "$repo_root/tools/bionic-libc-leaf-facade/imports/ndk-r28c-api35-arm64-libc.tsv" \
  "$LIBC_IMPORT_MANIFEST_SHA256"
expected_manifest="$build_root/expected-manifest.tsv"
mkdir -p "$build_root"
printf '%s\n' \
  $'symbol\ttype\tclass\tstatus\treason' \
  $'strtold\tFUNC\tB\tsupported\tAndroid-binary128-q0' \
  $'strtold_l\tFUNC\tB\tsupported\tAndroid-binary128-q0-locale-ignored' \
  $'wcstold\tFUNC\tB\tsupported\tAndroid-wchar32-to-binary128-q0' > "$expected_manifest"
cmp -s "$expected_manifest" "$script_dir/manifests/demand.tsv" || fail 'demand manifest drift'
printf '%s\n' \
  $'symbol\ttype\tclass\timplementation' \
  $'strtold\tFUNC\tB\tAAPCS64-q0+AOSP-gdtoa-strtorQ' \
  $'strtold_l\tFUNC\tB\tAAPCS64-q0+locale-ignored' \
  $'wcstold\tFUNC\tB\tAAPCS64-q0+ICU76-wchar32' > "$expected_manifest"
cmp -s "$expected_manifest" "$script_dir/manifests/imports.tsv" || fail 'provider import manifest drift'
for symbol in strtold strtold_l wcstold; do
  awk -F '\t' -v s="$symbol" '$1==s && $2=="FUNC" && $3=="B" {found=1} END {exit !found}' \
    "$repo_root/tools/bionic-libc-leaf-facade/imports/ndk-r28c-api35-arm64-libc.tsv" ||
    fail "missing pinned libc++ demand: $symbol"
done

float_archive="$repo_root/_build/bionic-float-conversion-facade/libdarwin-art-bionic-float-conversion.a"
[[ -f "$float_archive" ]] || fail 'run bionic-float-conversion-facade audit first'
for source in gdtoa.h gdtoaimp.h; do
  [[ -f "$repo_root/_aosp/bionic-float-conversion-facade/libc/upstream-openbsd/lib/libc/gdtoa/$source" ]] ||
    fail "missing shared pinned gdtoa source: $source"
done
ndk_root="${ANDROID_NDK_ROOT:-$HOME/Library/Android/sdk/ndk/$NDK_REVISION}"
toolchain="$ndk_root/toolchains/llvm/prebuilt/darwin-x86_64/bin"
android_cc="$toolchain/aarch64-linux-android${ANDROID_API}-clang"
readelf="$toolchain/llvm-readelf"
objdump="$toolchain/llvm-objdump"
for tool in "$android_cc" "$readelf" "$objdump"; do [[ -x "$tool" ]] || fail "missing $tool"; done
check_hash "$ndk_root/source.properties" "$NDK_SOURCE_PROPERTIES_SHA256"
ndk_libcxx="$ndk_root/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/lib/aarch64-linux-android/libc++_shared.so"
check_hash "$ndk_libcxx" "$NDK_ARM64_LIBCXX_SHARED_SHA256"
[[ "$(stat -f %z "$ndk_libcxx")" == "$NDK_ARM64_LIBCXX_SHARED_SIZE" ]] || fail 'NDK libc++ size drift'
libcxx_undefined="$($toolchain/llvm-nm -D -u "$ndk_libcxx")"
for symbol in strtold strtold_l wcstold; do
  grep -E " U ${symbol}@LIBC$" <<< "$libcxx_undefined" >/dev/null ||
    fail "pinned libc++ no longer demands $symbol@LIBC"
done
"$android_cc" -std=c17 -Wall -Wextra -Werror -fsyntax-only "$script_dir/probes/abi.c"

mkdir -p "$build_root"
fixture="$build_root/libbionic_binary128_fixture.so"
"$android_cc" -std=c17 -O2 -Wall -Wextra -Werror -fno-builtin -fPIC -shared \
  -nostdlib -Wl,--no-undefined -Wl,--version-script,"$script_dir/probes/exports.map" \
  -Wl,-z,norelro -Wl,--hash-style=both \
  "$script_dir/probes/fixture.c" -lc -o "$fixture"
imports="$($readelf --dyn-syms --wide "$fixture")"
for symbol in __errno strtold strtold_l wcstold; do
  grep -E " UND[[:space:]]+${symbol}@LIBC( |$)" <<< "$imports" >/dev/null || fail "missing ELF import $symbol@LIBC"
done
[[ "$(grep -Ec ' UND[[:space:]]+[^ ]+@(LIBC|LIBC_N)( |$)' <<< "$imports")" == 4 ]] || fail 'unexpected Android imports'
disassembly="$($objdump -d "$fixture")"
for symbol in strtold strtold_l wcstold; do
  grep -E "bl.*<${symbol}" <<< "$disassembly" >/dev/null || fail "fixture does not call $symbol"
done
grep -E 'str[[:space:]]+q0' <<< "$disassembly" >/dev/null || fail 'fixture does not observe q0 binary128 result'

runtime_root="$build_root/runtime"
rm -rf "$runtime_root"
mkdir -p "$runtime_root/data" "$runtime_root/tzdata" "$runtime_root/i18n/etc/icu"
ln -s "$repo_root/_build/icu-foundation/runtime/i18n/etc/icu/icudt76l.dat" \
  "$runtime_root/i18n/etc/icu/icudt76l.dat"
run_with_icu() {
  ANDROID_DATA="$runtime_root/data" ANDROID_TZDATA_ROOT="$runtime_root/tzdata" \
    ANDROID_I18N_ROOT="$runtime_root/i18n" "$@"
}

cargo fmt --manifest-path "$script_dir/Cargo.toml" -- --check
CARGO_TARGET_DIR="$build_root/production-target" cargo build --quiet --lib \
  --manifest-path "$script_dir/Cargo.toml"
CARGO_TARGET_DIR="$build_root/clippy" cargo clippy --quiet --all-targets --features audit \
  --manifest-path "$script_dir/Cargo.toml" -- -D warnings

archive="$(CARGO_TARGET_DIR="$build_root/target" cargo build --quiet --features audit \
  --message-format=json --manifest-path "$script_dir/Cargo.toml" |
  python3 -c 'import json, sys
paths = set()
for line in sys.stdin:
    event = json.loads(line)
    if event.get("reason") == "build-script-executed" and "/bionic-binary128-conversion-facade#" in event.get("package_id", ""):
        paths.add(event["out_dir"] + "/libdarwin_art_bionic_binary128_conversion.a")
if len(paths) != 1:
    raise SystemExit("expected exactly one current binary128 provider build-script output")
print(next(iter(paths)))')"
[[ -f "$archive" ]] || fail 'product archive missing'
cp "$archive" "$build_root/libdarwin-art-bionic-binary128-conversion.a"
members="$(ar -t "$archive" | grep -v '^__.SYMDEF')"
[[ "$(wc -l <<< "$members" | tr -d ' ')" == 3 ]] || fail 'archive must have exactly 3 members'
definitions="$(nm -gU "$archive")"
for symbol in darwin_art_bionic_strtold darwin_art_bionic_strtold_l darwin_art_bionic_wcstold darwin_art_aosp_strtorQ; do
  grep -F " _$symbol" <<< "$definitions" >/dev/null || fail "missing definition: $symbol"
done
if grep -E ' (_strtold|_strtold_l|_wcstold|___strtorQ)$' <<< "$definitions" >/dev/null; then
  fail 'unprefixed binary128 owner escaped'
fi
for duplicate in darwin_art_bionic_malloc darwin_art_bionic_free darwin_art_bionic___errno \
                 darwin_art_bionic_errno_store android_icu_init u_hasBinaryProperty __strtodg; do
  if grep -E " [TDS] _${duplicate}$" <<< "$definitions" >/dev/null; then
    fail "external allocator/errno/ICU/common-gdtoa owner embedded: $duplicate"
  fi
done
undefined="$(nm -u "$archive" | awk '{print $NF}' | sort -u)"
for provider in _darwin_art_bionic_malloc_result _darwin_art_bionic_free \
                _darwin_art_bionic_errno_store ___strtodg __Z16android_icu_initv \
                _u_hasBinaryProperty_76; do
  grep -Fx "$provider" <<< "$undefined" >/dev/null || fail "missing explicit provider edge: $provider"
done
if grep -E '^_(strtold|strtold_l|wcstold|dlopen|dlsym)$' <<< "$undefined" >/dev/null; then
  fail 'host dynamic/long-double fallback entered archive'
fi
otool -tvV "$archive" | grep -E 'ldr[[:space:]]+q0, \[sp, #0x10\]' >/dev/null ||
  fail 'AAPCS64 q0 load missing'
if rg -n '(^|[^A-Za-z_])long double([^A-Za-z_]|$)' "$script_dir/src/provider.cc" >/dev/null; then
  fail 'Darwin long double entered provider implementation'
fi

sdk="$(xcrun --sdk macosx --show-sdk-path)"
host_cxx="$(xcrun --find clang++)"
icu_root="$repo_root/_build/icu-foundation"
allocator_src="$repo_root/tools/bionic-libc-allocator-facade/src/allocator.c"
allocator_options_src="$repo_root/tools/bionic-libc-allocator-facade/src/allocator_options.c"
allocator_inc="$repo_root/tools/bionic-libc-allocator-facade/include"
check "$allocator_src" "$ALLOCATOR_SOURCE_SHA256"
check "$allocator_options_src" "$ALLOCATOR_OPTIONS_SOURCE_SHA256"
for sanitizer in address undefined; do
  san_dir="$build_root/differential-$sanitizer"
  mkdir -p "$san_dir"
  clang -arch arm64 -isysroot "$sdk" -std=c17 -O1 -Wall -Wextra -Werror \
    -fsanitize="$sanitizer" -fno-omit-frame-pointer -I"$allocator_inc" \
    -c "$allocator_src" -o "$san_dir/allocator.o"
  clang -arch arm64 -isysroot "$sdk" -std=c17 -O1 -Wall -Wextra -Werror \
    -fsanitize="$sanitizer" -fno-omit-frame-pointer -I"$allocator_inc" \
    -c "$allocator_options_src" -o "$san_dir/allocator_options.o"
  "$host_cxx" -arch arm64 -isysroot "$sdk" -std=c++20 -O1 -Wall -Wextra -Werror \
    -fsanitize="$sanitizer" -fno-omit-frame-pointer \
    -I"$repo_root/tools/bionic-errno-tls/include" \
    "$script_dir/probes/differential.cc" \
    -Wl,-force_load,"$archive" "$san_dir/allocator.o" "$san_dir/allocator_options.o" "$float_archive" \
    -Wl,-force_load,"$icu_root/libandroidicuinit-darwin.a" \
    "$icu_root/libicuuc-common-darwin.a" "$icu_root/libicuuc-stubdata-darwin.a" \
    -o "$san_dir/differential"
  run_with_icu "$san_dir/differential"
done

for sanitizer in address undefined; do
  target="$build_root/cargo-$sanitizer"
  run_with_icu env BIONIC_BINARY128_C_SANITIZER="$sanitizer" CARGO_TARGET_DIR="$target" \
    cargo run --quiet --features audit --manifest-path "$script_dir/Cargo.toml" -- "$fixture" >/dev/null
done

run_with_icu env CARGO_TARGET_DIR="$build_root/target" cargo run --quiet --features audit \
  --manifest-path "$script_dir/Cargo.toml" -- "$fixture"
runner="$build_root/target/debug/bionic-binary128-conversion-facade"
[[ -x "$runner" ]] || fail 'runner missing'
if otool -L "$runner" | grep -Ei '(homebrew|/opt/|libicu|libandroidicu|libgcc|quadmath)' >/dev/null; then
  fail 'dynamic host ICU/binary128 fallback linked'
fi

git -C "$repo_root" diff --quiet -- "$script_dir" || fail 'tracked source diff in standalone scope'
git -C "$repo_root" diff --cached --quiet -- "$script_dir" || fail 'staged source diff in standalone scope'
while IFS= read -r file; do
  relative="${file#"$repo_root/"}"
  if ! git -C "$repo_root" ls-files --error-unmatch "$relative" >/dev/null 2>&1 &&
     ! git -C "$repo_root" ls-files --others --exclude-standard -- "$relative" | grep -Fx "$relative" >/dev/null; then
    fail "ignored/unaccounted standalone file: $relative"
  fi
  whitespace_status=0
  git -C "$repo_root" diff --no-index --check -- /dev/null "$file" >/dev/null 2>&1 ||
    whitespace_status=$?
  [[ "$whitespace_status" == 0 || "$whitespace_status" == 1 ]] ||
    fail "whitespace error: $relative"
done < <(find "$script_dir" -type f | sort)

echo "bionic-binary128-conversion-facade: PASS demand=3 archive=3-members AndroidELF=3+errno q0=AAPCS64 AOSP-differential known-bits=+/-zero+subnormal+normal+max+overflow+inf+nan rounding=4 endptr locale-ignore wchar32=ICU76 threads=8x500 host-long-double=0 host-errno+fenv=preserved C-ASan C-UBSan closed=libc.so@LIBC"
