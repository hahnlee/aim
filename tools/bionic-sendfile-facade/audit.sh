#!/bin/bash
set -euo pipefail
export LC_ALL=C

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"
# shellcheck disable=SC1091
source "$here/sources.lock"
fail() { echo "bionic-sendfile-facade: FAIL $*" >&2; exit 3; }
sha() { shasum -a 256 "$1" | awk '{print $1}'; }
check() { [[ "$(sha "$1")" == "$2" ]] || fail "source drift: $1"; }

git -C "$root" diff --check -- tools/bionic-sendfile-facade || fail 'tracked whitespace'
git -C "$root" diff --cached --check -- tools/bionic-sendfile-facade || fail 'staged whitespace'
while IFS= read -r -d '' source_file; do
  set +e
  whitespace="$(git -C "$root" diff --no-index --check /dev/null "$source_file" 2>&1)"
  status=$?
  set -e
  [[ -z "$whitespace" && $status -le 1 ]] || fail "untracked whitespace: $source_file"
done < <(git -C "$root" ls-files --others --exclude-standard -z -- tools/bionic-sendfile-facade)
if find "$here" -type d -name target -print -quit | grep -q .; then fail 'source-tree target escaped'; fi

check "$here/include/aim_bionic_sendfile.h" "$HEADER_SHA256"
check "$here/src/sendfile.cc" "$PROVIDER_SHA256"
check "$here/probes/abi.c" "$ABI_SHA256"
check "$here/probes/fixture.c" "$FIXTURE_SHA256"
check "$here/probes/exports.map" "$EXPORTS_SHA256"
check "$here/probes/elf_runner.cc" "$RUNNER_SHA256"
check "$here/probes/lifecycle.cc" "$LIFECYCLE_SHA256"
check "$here/manifests/imports.tsv" "$IMPORTS_SHA256"
check "$here/manifests/callsites.tsv" "$CALLSITES_SHA256"
check "$here/upstream-sources.tsv" "$UPSTREAM_SOURCES_SHA256"
check "$here/README.md" "$README_SHA256"
check "$root/tools/bionic-libc-leaf-facade/imports/ndk-r28c-api35-arm64-libc.tsv" \
  "$LIBC_IMPORT_MANIFEST_SHA256"

tmp="$(mktemp -d "${TMPDIR:-/tmp}/bionic-sendfile.XXXXXX")"
trap 'find "$tmp" -depth -delete' EXIT
source_root="$root/_aosp/bionic-sendfile-facade"
while IFS=$'\t' read -r project revision relative size expected; do
  [[ "$project" != project ]] || continue
  destination="$source_root/$project/$relative"
  if [[ ! -f "$destination" ]]; then
    mkdir -p "$(dirname "$destination")"
    staged="$(mktemp "${TMPDIR:-/tmp}/bionic-sendfile-source.XXXXXX")"
    curl -fsSL "https://android.googlesource.com/$project/+/$revision/$relative?format=TEXT" |
      base64 -D > "$staged"
    [[ "$(stat -f %z "$staged")" == "$size" && "$(sha "$staged")" == "$expected" ]] ||
      fail "download provenance: $relative"
    mv "$staged" "$destination"
  fi
  [[ "$(stat -f %z "$destination")" == "$size" && "$(sha "$destination")" == "$expected" ]] ||
    fail "upstream drift: $relative"
done < "$here/upstream-sources.tsv"

operations="$source_root/toolchain/llvm-project/libcxx/src/filesystem/operations.cpp"
python3 - "$operations" <<'PY'
import sys
text = open(sys.argv[1]).read()
body = text[text.index('bool copy_file_impl(FileDescriptor& read_fd'):]
body = body[:body.index('#elif defined(_LIBCPP_FILESYSTEM_USE_COPYFILE)')]
for token in ('size_t count = read_fd.get_stat().st_size;',
              '::sendfile(write_fd.fd, read_fd.fd, nullptr, count)',
              'count -= res;', 'while (count > 0)'):
    assert token in body, token
print('bionic-sendfile-facade: LLVM source PASS exact nullptr+remaining loop')
PY

ndk="${ANDROID_NDK_ROOT:-${ANDROID_HOME:-$HOME/Library/Android/sdk}/ndk/$NDK_REVISION}"
tc="$ndk/toolchains/llvm/prebuilt/darwin-x86_64/bin"
android_cc="$tc/aarch64-linux-android${ANDROID_API}-clang"
readelf="$tc/llvm-readelf"
objdump="$tc/llvm-objdump"
libcxx="$tc/../sysroot/usr/lib/aarch64-linux-android/libc++_shared.so"
libc="$tc/../sysroot/usr/lib/aarch64-linux-android/35/libc.so"
check "$ndk/source.properties" "$NDK_SOURCE_PROPERTIES_SHA256"
check "$tc/../sysroot/usr/include/sys/sendfile.h" "$NDK_SENDFILE_HEADER_SHA256"
[[ "$(stat -f %z "$libcxx")" == "$NDK_LIBCXX_SIZE" ]] || fail 'libc++ size drift'
check "$libcxx" "$NDK_LIBCXX_SHA256"
check "$libc" "$NDK_LIBC_SHA256"

awk -F '\t' 'NR>1&&$1=="sendfile"{print $1"\t"$2"\t"$3"\t"$4}' \
  "$root/tools/bionic-libc-leaf-facade/imports/ndk-r28c-api35-arm64-libc.tsv" > "$tmp/demand"
diff -u <(tail -n +2 "$here/manifests/imports.tsv") "$tmp/demand" || fail 'demand drift'
"$readelf" --dyn-syms --wide "$libcxx" |
  awk '$4=="FUNC"&&$5=="GLOBAL"&&$7=="UND"&&$8=="sendfile@LIBC"{n++}END{exit n!=1}' ||
  fail 'libc++ sendfile import'
"$readelf" --dyn-syms --wide "$libc" |
  awk '$4=="FUNC"&&$5=="GLOBAL"&&$8=="sendfile@@LIBC"{n++}END{exit n!=1}' ||
  fail 'libc sendfile export'
"$objdump" -d --no-show-raw-insn "$libcxx" > "$tmp/libcxx.disassembly"
python3 - "$tmp/libcxx.disassembly" "$here/manifests/callsites.tsv" <<'PY'
import csv, re, sys
text = open(sys.argv[1]).read()
expected = [row['address'][2:] for row in csv.DictReader(open(sys.argv[2]), delimiter='\t')]
calls = re.findall(r'^\s*([0-9a-f]+):\s+bl\s+0x12b5f8 <sendfile@plt>$', text, re.M)
assert calls == expected, (calls, expected)
start = text.index('0000000000121388')
window = text[start:start + 900]
for token in ('ldr\tw0, [x22, #0x8]', 'ldr\tw1, [x20, #0x8]',
              'mov\tx2, xzr', 'mov\tx3, x21'):
    assert token in window, (token, window)
print('bionic-sendfile-facade: libc++ disassembly PASS callsite=1 x2=null')
PY

"$android_cc" -std=c17 -Wall -Wextra -Werror -S -emit-llvm \
  "$here/probes/abi.c" -o "$tmp/android.ll"
grep -F '@sendfile_signature' "$tmp/android.ll" >/dev/null || fail 'Android ABI probe'

host_cc="$(xcrun --find clang)"
host_cxx="$(xcrun --find clang++)"
sdk="$(xcrun --sdk macosx --show-sdk-path)"
includes=(-I"$here/include" -I"$root/tools/bionic-errno-tls/include")
cxxflags=(-arch arm64 -isysroot "$sdk" -std=c++20 -O2 -Wall -Wextra -Werror -Wpedantic)
"$host_cxx" "${cxxflags[@]}" "${includes[@]}" -c "$here/src/sendfile.cc" -o "$tmp/sendfile.o"
"$host_cc" -arch arm64 -isysroot "$sdk" -std=c17 -O2 -Wall -Wextra -Werror \
  -I"$root/tools/bionic-errno-tls/include" -I"$root/tools/bionic-errno-tls/generated" \
  -c "$root/tools/bionic-errno-tls/src/errno_tls.c" -o "$tmp/errno.o"
if nm -u "$tmp/sendfile.o" | awk '{print $NF}' | grep -E '^_(sendfile|syscall|dlsym|dlopen)$' >/dev/null; then
  fail 'host sendfile/syscall fallback escaped'
fi
for symbol in sendfile sendfile_activate sendfile_deactivate sendfile_resolve; do
  nm -gU "$tmp/sendfile.o" | grep -F " _aim_bionic_$symbol" >/dev/null || fail "definition: $symbol"
done

fixture="$tmp/libbionic_sendfile_fixture.so"
"$android_cc" -std=c17 -O2 -fno-builtin -fPIC -fno-stack-protector \
  -U_FORTIFY_SOURCE -D_FORTIFY_SOURCE=0 -Wall -Wextra -Werror -shared -nostdlib \
  -fuse-ld=lld -Wl,--build-id=none -Wl,--hash-style=sysv -Wl,-z,now \
  -Wl,-z,norelro -Wl,-z,max-page-size=16384 \
  -Wl,-soname,libbionic_sendfile_fixture.so \
  -Wl,--version-script,"$here/probes/exports.map" "$here/probes/fixture.c" -lc -o "$fixture"
"$readelf" -h "$fixture" | grep -F 'Machine:                           AArch64' >/dev/null || fail 'fixture machine'
"$readelf" --dyn-syms --wide "$fixture" |
  awk '$7=="UND"&&$8!=""{n=$8;sub(/@.*/,"",n);print n}' | sort -u > "$tmp/fixture-imports"
printf '%s\n' __errno sendfile | sort > "$tmp/expected-imports"
diff -u "$tmp/expected-imports" "$tmp/fixture-imports" || fail 'fixture import drift'
"$readelf" --dyn-syms --wide "$fixture" |
  awk '$7=="UND"&&$8!=""&&$8!~/@LIBC/{bad=1}END{exit bad}' || fail 'fixture version drift'

CARGO_TARGET_DIR="$tmp/loader-target" cargo build --quiet --release \
  --manifest-path "$root/crates/aim-elf-loader/Cargo.toml"
san=(-fsanitize=address,undefined -fno-omit-frame-pointer)
"$host_cxx" -arch arm64 -isysroot "$sdk" -std=c++20 -O1 -g -Wall -Wextra -Werror -Wpedantic \
  "${san[@]}" "${includes[@]}" -c "$here/src/sendfile.cc" -o "$tmp/sendfile-san.o"
"$host_cc" -arch arm64 -isysroot "$sdk" -std=c17 -O1 -g -Wall -Wextra -Werror \
  "${san[@]}" -I"$root/tools/bionic-errno-tls/include" -I"$root/tools/bionic-errno-tls/generated" \
  -c "$root/tools/bionic-errno-tls/src/errno_tls.c" -o "$tmp/errno-san.o"
"$host_cxx" -arch arm64 -isysroot "$sdk" -std=c++20 -O1 -g -Wall -Wextra -Werror -Wpedantic \
  "${san[@]}" "${includes[@]}" -I"$root/crates/aim-elf-loader/include" \
  "$here/probes/elf_runner.cc" "$tmp/sendfile-san.o" "$tmp/errno-san.o" \
  "$tmp/loader-target/release/libaim_elf_loader.a" -framework Security -o "$tmp/elf-runner"
ASAN_OPTIONS=detect_leaks=0:halt_on_error=1 UBSAN_OPTIONS=halt_on_error=1 \
  "$tmp/elf-runner" "$fixture"

tsan=(-fsanitize=thread -fno-omit-frame-pointer)
"$host_cc" -arch arm64 -isysroot "$sdk" -std=c17 -O1 -g -Wall -Wextra -Werror \
  "${tsan[@]}" -I"$root/tools/bionic-errno-tls/include" \
  -I"$root/tools/bionic-errno-tls/generated" \
  -c "$root/tools/bionic-errno-tls/src/errno_tls.c" -o "$tmp/errno-tsan.o"
"$host_cxx" -arch arm64 -isysroot "$sdk" -std=c++20 -O1 -g -Wall -Wextra -Werror -Wpedantic \
  "${tsan[@]}" "${includes[@]}" "$here/src/sendfile.cc" "$here/probes/lifecycle.cc" \
  "$tmp/errno-tsan.o" -o "$tmp/lifecycle-tsan"
TSAN_OPTIONS=halt_on_error=1 "$tmp/lifecycle-tsan"

ar rcs "$tmp/libaim-bionic-sendfile.a" "$tmp/sendfile.o"
mkdir -p "$root/_build/bionic-sendfile-facade"
cp "$tmp/libaim-bionic-sendfile.a" "$fixture" "$root/_build/bionic-sendfile-facade/"
echo 'bionic-sendfile-facade: PASS import=1 callsite=1 actual-ELF partial+EOF+offset+errno host-sendfile=0 ASan+UBSan+TSan'
