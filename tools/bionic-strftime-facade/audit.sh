#!/bin/bash
set -euo pipefail
export LC_ALL=C

dir="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$dir/../.." && pwd)"
# shellcheck disable=SC1091
source "$dir/sources.lock"

fail() { echo "bionic-strftime-facade: $*" >&2; exit 3; }
missing() { echo "bionic-strftime-facade: missing $*" >&2; exit 2; }
sha() { shasum -a 256 "$1" | awk '{print $1}'; }
check() { [[ "$(sha "$1")" == "$2" ]] || fail "hash: $1"; }
clean() {
  git -C "$root" diff --check -- tools/bionic-strftime-facade ||
    fail 'tracked diff check'
  git -C "$root" diff --cached --check -- tools/bionic-strftime-facade ||
    fail 'staged diff check'
  while IFS= read -r -d '' file; do
    set +e
    whitespace="$(git -C "$root" diff --no-index --check /dev/null "$file" 2>&1)"
    status=$?
    set -e
    [[ -z "$whitespace" ]] || fail "untracked whitespace: $file: $whitespace"
    [[ $status -le 1 ]] || fail "could not diff-check: $file"
  done < <(git -C "$root" ls-files --others --exclude-standard -z -- \
    tools/bionic-strftime-facade)
}

clean
task_tmp="$(mktemp -d "${TMPDIR:-/tmp}/bionic-strftime.XXXXXX")"
trap 'find "$task_tmp" -depth -delete' EXIT

check "$dir/README.md" "$README_SHA256"
check "$dir/audit.sh" "$AUDIT_SHA256"
check "$dir/include/aim_bionic_strftime.h" "$HEADER_SHA256"
check "$dir/manifests/imports.tsv" "$IMPORTS_SHA256"
check "$dir/manifests/callsites.tsv" "$CALLSITES_SHA256"
check "$dir/manifests/formats.tsv" "$FORMATS_SHA256"
check "$dir/probes/elf_runner.cc" "$ELF_RUNNER_SHA256"
check "$dir/probes/exports.map" "$EXPORTS_SHA256"
check "$dir/probes/fixture.c" "$FIXTURE_SHA256"
check "$dir/src/provider.c" "$PROVIDER_SHA256"
check "$dir/src/upstream_shim.h" "$UPSTREAM_SHIM_SHA256"
check "$dir/upstream-sources.tsv" "$UPSTREAM_SOURCES_SHA256"

source_root="$root/_aosp/bionic-strftime-facade"
while IFS=$'\t' read -r project revision relative size expected; do
  [[ "$project" != project ]] || continue
  destination="$source_root/$project/$relative"
  if [[ ! -f "$destination" ]]; then
    mkdir -p "$(dirname "$destination")"
    staged="$(mktemp "${TMPDIR:-/tmp}/bionic-strftime-source.XXXXXX")"
    curl -fsSL \
      "https://android.googlesource.com/$project/+/$revision/$relative?format=TEXT" |
      base64 -D > "$staged"
    [[ "$(stat -f %z "$staged")" == "$size" &&
       "$(sha "$staged")" == "$expected" ]] ||
      fail "download provenance: $project/$relative"
    mv "$staged" "$destination"
  fi
  [[ "$(stat -f %z "$destination")" == "$size" &&
     "$(sha "$destination")" == "$expected" ]] ||
    fail "source drift: $project/$relative"
done < "$dir/upstream-sources.tsv"

bionic="$source_root/platform/bionic"
llvm="$source_root/toolchain/llvm-project"
python3 - "$bionic/libc/tzcode/strftime.c" "$llvm/libcxx/src/locale.cpp" <<'PY'
from pathlib import Path
import sys

strftime = Path(sys.argv[1]).read_text()
locale = Path(sys.argv[2]).read_text()
assert 'Just call strftime, as only the C locale is supported.' in strftime
assert 'struct lc_time_T const *Locale = &C_time_locale;' in strftime
assert 'case \'_\':' in strftime and 'case \'#\':' in strftime
assert 'case \'P\':' in strftime and '#define FORCE_LOWER_CASE' in strftime
assert '_safe_tm_zone(t)' in strftime and 'diff = t->TM_GMTOFF;' in strftime
assert 'mkt = mktime64(&tm);' in strftime
assert locale.count('strftime_l(') == 15
assert "char fmt[] = {'%', __fmt, __mod, 0};" in locale
assert "char f[3] = {0};" in locale and "f[1]      = fmt;" in locale
print('bionic-strftime-facade: source semantics PASS C-locale+dynamic-format+timezone-fields')
PY

ndk="${ANDROID_NDK_ROOT:-${ANDROID_HOME:-$HOME/Library/Android/sdk}/ndk/$NDK_REVISION}"
tc="$ndk/toolchains/llvm/prebuilt/darwin-x86_64/bin"
android_cc="$tc/aarch64-linux-android${ANDROID_API}-clang"
readelf="$tc/llvm-readelf"
objdump="$tc/llvm-objdump"
libcxx="$tc/../sysroot/usr/lib/aarch64-linux-android/libc++_shared.so"
libc="$tc/../sysroot/usr/lib/aarch64-linux-android/35/libc.so"
ndk_include="$tc/../sysroot/usr/include"
for input in "$android_cc" "$readelf" "$objdump" "$libcxx" "$libc"; do
  [[ -e "$input" ]] || missing "$input"
done
check "$ndk/source.properties" "$NDK_SOURCE_PROPERTIES_SHA256"
[[ "$(stat -f %z "$libcxx")" == "$NDK_LIBCXX_SIZE" ]] ||
  fail 'libc++ size drift'
check "$libcxx" "$NDK_LIBCXX_SHA256"
check "$libc" "$NDK_LIBC_SHA256"
check "$ndk_include/time.h" "$NDK_TIME_HEADER_SHA256"
check "$ndk_include/locale.h" "$NDK_LOCALE_HEADER_SHA256"

master="$root/tools/bionic-libc-leaf-facade/imports/ndk-r28c-api35-arm64-libc.tsv"
check "$master" "$LIBC_IMPORT_MANIFEST_SHA256"
awk -F '\t' 'NR==1 || $1=="strftime_l"' "$master" > "$task_tmp/master-demand"
diff -u "$task_tmp/master-demand" "$dir/manifests/imports.tsv" ||
  fail 'libc++ demand drift'
"$readelf" --dyn-syms --wide "$libcxx" |
  awk '$4=="FUNC"&&$5=="GLOBAL"&&$7=="UND"&&$8=="strftime_l@LIBC"{n++}END{exit n!=1}' ||
  fail 'real libc++ strftime_l import'
"$readelf" --dyn-syms --wide "$libc" |
  awk '$4=="FUNC"&&$5=="GLOBAL"&&$8=="strftime_l@@LIBC"{n++}END{exit n!=1}' ||
  fail 'API 35 arm64 libc strftime_l ABI'

"$objdump" -d --no-show-raw-insn "$libcxx" > "$task_tmp/libcxx.disassembly"
python3 - "$task_tmp/libcxx.disassembly" "$dir/manifests/callsites.tsv" <<'PY'
import csv
import re
import sys

text = open(sys.argv[1]).read()
rows = list(csv.DictReader(open(sys.argv[2]), delimiter='\t'))
calls = re.findall(
    r'^\s*([0-9a-f]+):\s+bl\s+0x[0-9a-f]+ <strftime_l@plt>$', text, re.M)
expected = [row['address'][2:] for row in rows]
assert calls == expected, (calls, expected)
assert len(rows) == 17
assert sum('dynamic-' in row['form'] for row in rows) == 5
print('bionic-strftime-facade: disassembly PASS callsites=17 dynamic=5')
PY

fixture="$task_tmp/libbionic_strftime_fixture.so"
"$android_cc" -std=c17 -O2 -fno-builtin -fPIC -fno-stack-protector \
  -U_FORTIFY_SOURCE -D_FORTIFY_SOURCE=0 -Wall -Wextra -Werror \
  -shared -nostdlib -fuse-ld=lld -Wl,--build-id=none \
  -Wl,--hash-style=sysv -Wl,-z,now -Wl,-z,norelro \
  -Wl,-z,max-page-size=16384 -Wl,-soname,libbionic_strftime_fixture.so \
  -Wl,--version-script,"$dir/probes/exports.map" \
  "$dir/probes/fixture.c" -lc -o "$fixture"
"$readelf" -h "$fixture" | grep -F 'Machine:                           AArch64' >/dev/null ||
  fail 'fixture machine'
"$readelf" --dyn-syms --wide "$fixture" |
  awk '$7=="UND"&&$8!=""{print $8}' > "$task_tmp/fixture-imports"
printf '%s\n' 'strftime_l@LIBC' > "$task_tmp/fixture-expected"
diff -u "$task_tmp/fixture-expected" "$task_tmp/fixture-imports" ||
  fail 'fixture import drift'

sdk="$(xcrun --sdk macosx --show-sdk-path)"
host_cc="$(xcrun --find clang)"
host_cxx="$(xcrun --find clang++)"
includes=(-I"$dir/include" -I"$root/tools/bionic-errno-tls/include")
cflags=(-arch arm64 -isysroot "$sdk" -std=c17 -O2 -Wall -Wextra -Werror -Wpedantic)
upstream_flags=(-U_FORTIFY_SOURCE -D_FORTIFY_SOURCE=0 -DHAVE_STRFTIME_L=1
  -D__BIONIC__=1 -D__LP64__=1 -DDEPRECATE_TWO_DIGIT_YEARS=0
  -include "$dir/src/upstream_shim.h")
"$host_cc" "${cflags[@]}" "${includes[@]}" \
  -c "$dir/src/provider.c" -o "$task_tmp/provider.o"
"$host_cc" "${cflags[@]}" "${upstream_flags[@]}" \
  -c "$bionic/libc/tzcode/strftime.c" -o "$task_tmp/upstream.o"
if nm -u "$task_tmp/upstream.o" |
    awk '{print $NF}' |
    grep -E '^_(strftime|strftime_l|mktime|tzset|tolower|toupper|islower|isupper|sprintf|__sprintf_chk|dlsym|dlopen)$' >/dev/null; then
  fail 'host formatter/timezone fallback escaped'
fi
nm -u "$task_tmp/provider.o" |
  grep -F '_aim_bionic_errno_store' >/dev/null ||
  fail 'Bionic errno route missing'
ar rcs "$task_tmp/libaim-bionic-strftime.a" \
  "$task_tmp/provider.o" "$task_tmp/upstream.o"

loader_target="$task_tmp/loader-target"
CARGO_TARGET_DIR="$loader_target" cargo build --quiet --release --lib \
  --manifest-path "$root/crates/aim-elf-loader/Cargo.toml"
san=(-fsanitize=address,undefined -fno-omit-frame-pointer)
"$host_cc" -arch arm64 -isysroot "$sdk" -std=c17 -O1 -g \
  -Wall -Wextra -Werror -Wpedantic "${san[@]}" "${includes[@]}" \
  -c "$dir/src/provider.c" -o "$task_tmp/provider-san.o"
"$host_cc" -arch arm64 -isysroot "$sdk" -std=c17 -O1 -g \
  -Wall -Wextra -Werror -Wpedantic "${san[@]}" "${upstream_flags[@]}" \
  -c "$bionic/libc/tzcode/strftime.c" -o "$task_tmp/upstream-san.o"
"$host_cc" -arch arm64 -isysroot "$sdk" -std=c17 -O1 -g \
  -Wall -Wextra -Werror "${san[@]}" \
  -I"$root/tools/bionic-errno-tls/include" \
  -I"$root/tools/bionic-errno-tls/generated" \
  -c "$root/tools/bionic-errno-tls/src/errno_tls.c" \
  -o "$task_tmp/errno-san.o"
"$host_cxx" -arch arm64 -isysroot "$sdk" -std=c++20 -O1 -g \
  -Wall -Wextra -Werror -Wpedantic "${san[@]}" "${includes[@]}" \
  -I"$root/crates/aim-elf-loader/include" \
  "$dir/probes/elf_runner.cc" "$task_tmp/provider-san.o" \
  "$task_tmp/upstream-san.o" "$task_tmp/errno-san.o" \
  "$loader_target/release/libaim_elf_loader.a" \
  -framework Security -o "$task_tmp/elf-runner"
ASAN_OPTIONS=detect_leaks=0:halt_on_error=1 UBSAN_OPTIONS=halt_on_error=1 \
  "$task_tmp/elf-runner" "$fixture"

mkdir -p "$root/_build/bionic-strftime-facade"
cp "$task_tmp/libaim-bionic-strftime.a" \
  "$root/_build/bionic-strftime-facade/"
cp "$fixture" "$root/_build/bionic-strftime-facade/"
clean
echo 'bionic-strftime-facade: PASS import=1 callsites=17 formats=full-C Android-ELF=yes timezone=fixed-offset errno=Bionic host-fallback=0 threads=8 ASan+UBSan'
