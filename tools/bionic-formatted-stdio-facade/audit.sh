#!/bin/bash
set -euo pipefail
export LC_ALL=C

dir="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$dir/../.." && pwd)"
# shellcheck disable=SC1091
source "$dir/sources.lock"

fail() { echo "bionic-formatted-stdio-facade: $*" >&2; exit 3; }
missing() { echo "bionic-formatted-stdio-facade: missing $*" >&2; exit 2; }
sha() { shasum -a 256 "$1" | awk '{print $1}'; }
check() { [[ "$(sha "$1")" == "$2" ]] || fail "hash: $1"; }
clean() {
  [[ -z "$(find "$dir" -type d -name target -print -quit)" ]] || fail 'local target exists'
  git -C "$root" diff --check -- tools/bionic-formatted-stdio-facade || fail 'tracked diff check'
  git -C "$root" diff --cached --check -- tools/bionic-formatted-stdio-facade || fail 'staged diff check'
  while IFS= read -r -d '' file; do
    set +e
    whitespace="$(git -C "$root" diff --no-index --check /dev/null "$file" 2>&1)"
    status=$?
    set -e
    [[ -z "$whitespace" ]] || fail "untracked whitespace: $file: $whitespace"
    [[ $status -le 1 ]] || fail "could not diff-check untracked file: $file"
  done < <(git -C "$root" ls-files --others --exclude-standard -z -- tools/bionic-formatted-stdio-facade)
}

clean
tmp="$(mktemp -d "${TMPDIR:-/tmp}/bionic-formatted-stdio.XXXXXX")"
cleanup() {
  if [[ "$tmp" == "${TMPDIR:-/tmp}"/bionic-formatted-stdio.* ]]; then
    find "$tmp" -depth -delete
  fi
}
trap cleanup EXIT

master="$root/tools/bionic-libc-leaf-facade/imports/ndk-r28c-api35-arm64-libc.tsv"
format_root="$root/tools/bionic-format-facade"
stdio_root="$root/tools/bionic-stdio-facade"
allocator_root="$root/tools/bionic-libc-allocator-facade"
errno_root="$root/tools/bionic-errno-tls"
check "$master" "$LIBC_IMPORT_MANIFEST_SHA256"
check "$format_root/sources.lock" "$FORMAT_LOCK_SHA256"
check "$format_root/src/format.cc" "$FORMAT_SOURCE_SHA256"
check "$format_root/src/aapcs64_entry.S" "$FORMAT_ENTRY_SHA256"
check "$stdio_root/sources.lock" "$STDIO_LOCK_SHA256"
check "$stdio_root/src/lib.rs" "$STDIO_SOURCE_SHA256"
check "$stdio_root/src/shims.c" "$STDIO_SHIMS_SHA256"
check "$stdio_root/probes/audit_dependency_shims.c" "$STDIO_AUDIT_SHIMS_SHA256"
check "$dir/upstream-sources.tsv" "$UPSTREAM_SOURCES_SHA256"
check "$dir/manifests/demand.tsv" "$DEMAND_SHA256"
check "$dir/manifests/imports.tsv" "$IMPORTS_SHA256"
check "$dir/include/aim_bionic_formatted_stdio.h" "$HEADER_SHA256"
check "$dir/src/provider.cc" "$PROVIDER_SHA256"
check "$dir/src/aapcs64_entry.S" "$ENTRY_SHA256"
check "$dir/probes/abi.c" "$ABI_SHA256"
check "$dir/probes/fixture.c" "$FIXTURE_SHA256"
check "$dir/probes/exports.map" "$EXPORTS_SHA256"
check "$dir/build.rs" "$BUILD_RS_SHA256"
check "$dir/Cargo.toml" "$CARGO_TOML_SHA256"
check "$dir/src/main.rs" "$MAIN_SHA256"
check "$dir/README.md" "$README_SHA256"

awk -F '\t' 'NR>1&&($1=="fprintf"||$1=="vfprintf"){print $1 "\t" $2 "\t" $3}' \
  "$master" | sort > "$tmp/master-demand"
tail -n +2 "$dir/manifests/demand.tsv" | cut -f1-3 | sort > "$tmp/locked-demand"
diff -u "$tmp/master-demand" "$tmp/locked-demand" || fail 'libc++ demand drift'
[[ "$(wc -l < "$tmp/master-demand" | tr -d ' ')" == 2 ]] || fail 'demand count drift'
tail -n +2 "$dir/manifests/imports.tsv" | cut -f1-3 | sort > "$tmp/imports"
diff -u "$tmp/master-demand" "$tmp/imports" || fail 'ownership manifest drift'

source_root="$root/_aosp/bionic-formatted-stdio-facade"
[[ "$(tail -n +2 "$dir/upstream-sources.tsv" | wc -l | tr -d ' ')" == "$BIONIC_SOURCE_COUNT" ]] ||
  fail 'upstream source count drift'
while IFS=$'\t' read -r relative size expected; do
  [[ "$relative" != path ]] || continue
  destination="$source_root/$relative"
  if [[ ! -f "$destination" ]]; then
    mkdir -p "$(dirname "$destination")"
    staged="$(mktemp "${TMPDIR:-/tmp}/bionic-formatted-stdio-source.XXXXXX")"
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
stdio=(r/'libc/stdio/stdio.cpp').read_text()
vf=(r/'libc/stdio/vfprintf.cpp').read_text()
common=(r/'libc/stdio/printf_common.h').read_text()
assert '"stdio/stdio.cpp"' in bp and '"stdio/vfprintf.cpp"' in bp
assert 'int fprintf(FILE* fp, const char* fmt, ...)' in stdio
assert 'CHECK_FP(fp);' in stdio
assert 'PRINTF_IMPL(vfprintf(fp, fmt, ap));' in stdio
assert 'int vfprintf(FILE* fp, const char* fmt, va_list ap)' in stdio
assert 'return __vfprintf(fp, fmt, ap);' in stdio
assert '#define FUNCTION_NAME __vfprintf' in vf
assert 'int FUNCTION_NAME(FILE* fp, const CHAR_TYPE* fmt0, va_list ap)' in vf
assert 'static int __sbprintf(FILE* fp, const CHAR_TYPE* fmt, va_list ap)' in common
print('bionic-formatted-stdio-facade: upstream=PASS fprintf-wrapper+vfprintf-lock+__vfprintf-core')
PY

ndk="${ANDROID_NDK_ROOT:-$HOME/Library/Android/sdk/ndk/$NDK_REVISION}"
tc="$ndk/toolchains/llvm/prebuilt/darwin-x86_64/bin"
android_cc="$tc/aarch64-linux-android${ANDROID_API}-clang"
readelf="$tc/llvm-readelf"
objdump="$tc/llvm-objdump"
libcxx="$ndk/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/lib/aarch64-linux-android/libc++_shared.so"
for input in "$ndk/source.properties" "$libcxx" "$android_cc" "$readelf"; do
  [[ -e "$input" ]] || missing "$input"
done
check "$ndk/source.properties" "$NDK_SOURCE_PROPERTIES_SHA256"
check "$libcxx" "$NDK_LIBCXX_SHARED_SHA256"
"$readelf" --dyn-syms --wide "$libcxx" |
  awk '$7=="UND"&&($8=="fprintf@LIBC"||$8=="vfprintf@LIBC"){print $8}' | sort > "$tmp/libcxx-demand"
printf '%s\n' fprintf@LIBC vfprintf@LIBC | sort > "$tmp/libcxx-expected"
diff -u "$tmp/libcxx-expected" "$tmp/libcxx-demand" || fail 'actual NDK libc++ demand drift'
"$android_cc" -std=c17 -Wall -Wextra -Werror -Wpedantic -S -emit-llvm \
  "$dir/probes/abi.c" -o "$tmp/android-abi.ll"
grep -F '%struct.__va_list = type { ptr, ptr, ptr, i32, i32 }' "$tmp/android-abi.ll" >/dev/null ||
  fail 'Android AArch64 va_list layout drift'

# The composed dependencies must pass independently before this owner is linked.
bash "$format_root/audit.sh" >/dev/null
bash "$stdio_root/audit.sh" >/dev/null

sdk="$(xcrun --sdk macosx --show-sdk-path)"
host_cc="$(xcrun --find clang)"
host_cxx="$(xcrun --find clang++)"
includes=(-I"$dir/include" -I"$format_root/include" -I"$stdio_root/include" \
  -I"$allocator_root/include" -I"$errno_root/include")
"$host_cxx" -arch arm64 -isysroot "$sdk" -std=c++20 -O2 -Wall -Wextra -Werror \
  -fno-builtin "${includes[@]}" -c "$dir/src/provider.cc" -o "$tmp/provider.o"
"$host_cc" -arch arm64 -isysroot "$sdk" -c "$dir/src/aapcs64_entry.S" -o "$tmp/entry.o"
ar rcs "$tmp/libaim-bionic-formatted-stdio.a" "$tmp/provider.o" "$tmp/entry.o"
file "$tmp/libaim-bionic-formatted-stdio.a" | grep -F 'current ar archive' >/dev/null ||
  fail 'product is not an archive'
[[ "$(ar -t "$tmp/libaim-bionic-formatted-stdio.a" | grep -vc '^__.SYMDEF')" == 2 ]] ||
  fail 'product archive member count drift'
definitions="$(nm -gU "$tmp/libaim-bionic-formatted-stdio.a" | awk '{print $NF}')"
for symbol in fprintf vfprintf formatted_stdio_resolve formatted_stdio_capability; do
  grep -Fx "_aim_bionic_$symbol" <<< "$definitions" >/dev/null || fail "missing owner: $symbol"
done
for foreign in malloc_result free errno_store vsnprintf fwrite fwrite_core stdio_fwrite_core \
               format_resolve stdio_resolve; do
  ! grep -Fx "_aim_bionic_$foreign" <<< "$definitions" >/dev/null ||
    fail "foreign owner embedded: $foreign"
done
undefined="$(nm -u "$tmp/provider.o" | awk '{print $NF}')"
for dependency in _aim_bionic_malloc_result _aim_bionic_free \
                  _aim_bionic_errno_store _aim_bionic_vsnprintf \
                  _aim_bionic_stdio_fflush_core _aim_bionic_stdio_fwrite_core; do
  grep -Fx "$dependency" <<< "$undefined" >/dev/null || fail "missing dependency seam: $dependency"
done
grep -Fx _aim_bionic_fprintf <<< "$undefined" >/dev/null ||
  fail 'resolver does not reference the AAPCS64 fprintf entry'
[[ "$(nm -gU "$tmp/entry.o" | awk '$2=="T"&&$3=="_aim_bionic_fprintf"{n++}END{print n+0}')" == 1 ]] ||
  fail 'AAPCS64 assembly does not uniquely own fprintf'
if grep -E '^_(fprintf|vfprintf|vsnprintf|fwrite|fflush)$' <<< "$undefined" >/dev/null; then
  fail 'Darwin stdio/format fallback escaped'
fi
"$objdump" -d "$tmp/entry.o" > "$tmp/entry.dis"
for instruction in 'sub[[:space:]]+sp, sp, #0xe0' 'add[[:space:]]+x4, sp, #0xe0'; do
  grep -E "$instruction" "$tmp/entry.dis" >/dev/null || fail "AAPCS64 capture drift: $instruction"
done
"$objdump" -r "$tmp/entry.o" | grep -E \
  'ARM64_RELOC_BRANCH26[[:space:]]+_aim_bionic_fprintf_captured' >/dev/null ||
  fail 'AAPCS64 capture helper relocation drift'

fixture="$tmp/libbionic_formatted_stdio_fixture.so"
"$android_cc" -std=c17 -O2 -fno-builtin -fPIC -fno-stack-protector \
  -U_FORTIFY_SOURCE -D_FORTIFY_SOURCE=0 -Wall -Wextra -Werror -Wpedantic \
  -shared -nostdlib -fuse-ld=lld -Wl,--build-id=none -Wl,--hash-style=sysv \
  -Wl,-z,now -Wl,-z,norelro -Wl,-z,max-page-size=16384 \
  -Wl,-soname,libbionic_formatted_stdio_fixture.so \
  -Wl,--version-script,"$dir/probes/exports.map" "$dir/probes/fixture.c" -lc -o "$fixture"
file "$fixture" | grep -F 'ELF 64-bit LSB shared object, ARM aarch64' >/dev/null ||
  fail 'fixture is not Android arm64 ELF'
"$readelf" --dyn-syms --wide "$fixture" |
  awk '$7=="UND"&&$8!=""{print $8}' | sort -u > "$tmp/fixture.imports"
printf '%s\n' __errno@LIBC fprintf@LIBC vfprintf@LIBC | sort > "$tmp/fixture.expected"
diff -u "$tmp/fixture.expected" "$tmp/fixture.imports" || fail 'Android ELF import drift'
"$readelf" -d "$fixture" | awk '$2=="(NEEDED)"{gsub(/\[|\]/,"",$5);print $5}' > "$tmp/needed"
printf '%s\n' libc.so > "$tmp/needed.expected"
diff -u "$tmp/needed.expected" "$tmp/needed" || fail 'Android ELF graph is not closed to libc.so'
for symbol in bionic_formatted_stdio_fixture_fprintf bionic_formatted_stdio_fixture_vfprintf \
              bionic_formatted_stdio_fixture_rejected bionic_formatted_stdio_fixture_capacity \
              bionic_formatted_stdio_fixture_foreign_file; do
  "$readelf" --dyn-syms --wide "$fixture" |
    awk -v symbol="$symbol" '$7!="UND"&&$8==symbol{found=1}END{exit !found}' || fail "fixture export: $symbol"
done

run_gate() {
  local mode="$1"
  local sanitizer="$2"
  local target="$tmp/cargo-$mode"
  if [[ -n "$sanitizer" ]]; then
    ASAN_OPTIONS=halt_on_error=1 UBSAN_OPTIONS=halt_on_error=1 \
      BIONIC_FORMATTED_STDIO_C_SANITIZER="$sanitizer" \
      BIONIC_STDIO_C_SANITIZER="$sanitizer" \
      CARGO_TARGET_DIR="$target" cargo run --quiet --manifest-path "$dir/Cargo.toml" -- "$fixture"
  else
    CARGO_TARGET_DIR="$target" cargo run --quiet --manifest-path "$dir/Cargo.toml" -- "$fixture"
  fi
}
run_gate normal ''
CARGO_TARGET_DIR="$tmp/cargo-normal" cargo clippy --quiet --manifest-path "$dir/Cargo.toml" -- -D warnings
run_gate asan address
run_gate ubsan undefined

binary="$tmp/cargo-normal/debug/bionic-formatted-stdio-facade"
[[ -x "$binary" ]] || fail 'missing composed gate executable'
all_symbols="$(nm -a "$binary")"
for owner in aim_bionic_malloc_result aim_bionic_free \
             aim_bionic_errno_store aim_bionic_stdio_fwrite_core \
             aim_bionic_vsnprintf aim_bionic_fprintf aim_bionic_vfprintf; do
  count="$(awk -v symbol="_$owner" '$2~/^[Tt]$/&&$3==symbol{n++}END{print n+0}' <<< "$all_symbols")"
  [[ "$count" == 1 ]] || fail "owner multiplicity $owner=$count"
done

mkdir -p "$root/_build/bionic-formatted-stdio-facade"
cp "$tmp/libaim-bionic-formatted-stdio.a" \
  "$root/_build/bionic-formatted-stdio-facade/libaim-bionic-formatted-stdio.a"
cp "$fixture" "$root/_build/bionic-formatted-stdio-facade/"
cargo fmt --manifest-path "$dir/Cargo.toml" -- --check
clean
echo 'bionic-formatted-stdio-facade: PASS demand=2 AndroidELF=fprintf+vfprintf AAPCS64=GP6+stack2/FP8+stack2 FILE=provider-local output<=1048576 semantic/provider-failure=precommit duplicate-owner=0 C-ASan C-UBSan closed=libc.so@LIBC'
