#!/bin/bash
set -euo pipefail
export LC_ALL=C

dir="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$dir/../.." && pwd)"
# shellcheck disable=SC1091
source "$dir/sources.lock"

fail() { echo "bionic-ioctl-facade: $*" >&2; exit 3; }
missing() { echo "bionic-ioctl-facade: missing $*" >&2; exit 2; }
sha() { shasum -a 256 "$1" | awk '{print $1}'; }
check() { [[ "$(sha "$1")" == "$2" ]] || fail "hash: $1"; }
clean() {
  git -C "$root" diff --check -- tools/bionic-ioctl-facade ||
    fail 'tracked diff check'
  git -C "$root" diff --cached --check -- tools/bionic-ioctl-facade ||
    fail 'staged diff check'
  while IFS= read -r -d '' file; do
    set +e
    whitespace="$(git -C "$root" diff --no-index --check /dev/null "$file" 2>&1)"
    status=$?
    set -e
    [[ -z "$whitespace" ]] || fail "untracked whitespace: $file: $whitespace"
    [[ $status -le 1 ]] || fail "could not diff-check: $file"
  done < <(git -C "$root" ls-files --others --exclude-standard -z -- \
    tools/bionic-ioctl-facade)
}

clean
tmp="$(mktemp -d "${TMPDIR:-/tmp}/bionic-ioctl.XXXXXX")"
trap 'find "$tmp" -depth -delete' EXIT

check "$dir/README.md" "$README_SHA256"
check "$dir/audit.sh" "$AUDIT_SHA256"
check "$dir/include/aim_bionic_ioctl.h" "$HEADER_SHA256"
check "$dir/manifests/imports.tsv" "$IMPORTS_SHA256"
check "$dir/manifests/callsites.tsv" "$CALLSITES_SHA256"
check "$dir/manifests/requests.tsv" "$REQUESTS_SHA256"
check "$dir/manifests/unsupported.tsv" "$UNSUPPORTED_SHA256"
check "$dir/upstream-sources.tsv" "$UPSTREAM_SOURCES_SHA256"
check "$dir/probes/abi.c" "$ABI_SHA256"
check "$dir/probes/elf_runner.cc" "$ELF_RUNNER_SHA256"
check "$dir/probes/exports.map" "$EXPORTS_SHA256"
check "$dir/probes/fixture.c" "$FIXTURE_SHA256"
check "$dir/probes/socket_broker_stub.cc" "$SOCKET_BROKER_STUB_SHA256"
check "$dir/src/aapcs64_entry.S" "$ENTRY_SHA256"
check "$dir/src/ioctl.cc" "$PROVIDER_SHA256"

source_root="$root/_aosp/bionic-ioctl-facade"
while IFS=$'\t' read -r project revision relative size expected; do
  [[ "$project" != project ]] || continue
  destination="$source_root/$project/$relative"
  if [[ ! -f "$destination" ]]; then
    mkdir -p "$(dirname "$destination")"
    staged="$(mktemp "${TMPDIR:-/tmp}/bionic-ioctl-source.XXXXXX")"
    curl -fsSL "https://android.googlesource.com/$project/+/$revision/$relative?format=TEXT" |
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

python3 - "$source_root" <<'PY'
from pathlib import Path
import sys
root = Path(sys.argv[1])
random = (root / 'toolchain/llvm-project/libcxx/src/random.cpp').read_text()
ioctl = (root / 'platform/bionic/libc/bionic/ioctl.cpp').read_text()
assert '::ioctl(__f_, RNDGETENTCNT, &ent) < 0' in random
assert 'if (ent < 0)' in random
assert 'std::numeric_limits<result_type>::digits' in random
assert 'int ioctl(int fd, int request, ...)' in ioctl
assert 'void* arg = va_arg(ap, void*)' in ioctl
assert 'return __ioctl(fd, request, arg);' in ioctl
print('bionic-ioctl-facade: source semantics PASS random-device+bionic-varargs')
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
check "$ndk_include/bits/ioctl.h" "$NDK_IOCTL_HEADER_SHA256"
check "$ndk_include/linux/random.h" "$NDK_RANDOM_HEADER_SHA256"

cat > "$tmp/request.c" <<'EOF'
#include <linux/random.h>
_Static_assert(RNDGETENTCNT == 0x80045200U, "Android RNDGETENTCNT");
_Static_assert(sizeof(int) == 4, "Android int size");
int main(void) { return 0; }
EOF
"$android_cc" -std=c17 -Wall -Wextra -Werror -c "$tmp/request.c" \
  -o "$tmp/request.o"

master="$root/tools/bionic-libc-leaf-facade/imports/ndk-r28c-api35-arm64-libc.tsv"
check "$master" "$LIBC_IMPORT_MANIFEST_SHA256"
awk -F '\t' 'NR>1 && $1=="ioctl"{print $1 "\t" $2 "\t" $3 "\t" $4}' \
  "$master" > "$tmp/master-demand"
tail -n +2 "$dir/manifests/imports.tsv" > "$tmp/locked-demand"
diff -u "$tmp/master-demand" "$tmp/locked-demand" ||
  fail 'libc++ demand drift'
[[ "$(wc -l < "$tmp/master-demand" | tr -d ' ')" == 1 ]] ||
  fail 'ioctl demand count'
"$readelf" --dyn-syms --wide "$libcxx" |
  awk '$4=="FUNC"&&$5=="GLOBAL"&&$7=="UND"&&$8=="ioctl@LIBC"{n++}END{exit n!=1}' ||
  fail 'real libc++ ioctl import'
"$readelf" --dyn-syms --wide "$libc" |
  awk '$4=="FUNC"&&$5=="GLOBAL"&&$8=="ioctl@@LIBC"{n++}END{exit n!=1}' ||
  fail 'API 35 arm64 libc ioctl ABI'

"$objdump" -d --no-show-raw-insn "$libcxx" > "$tmp/libcxx.disassembly"
python3 - "$tmp/libcxx.disassembly" "$dir/manifests/callsites.tsv" <<'PY'
import csv, re, sys
text = open(sys.argv[1]).read()
rows = list(csv.DictReader(open(sys.argv[2]), delimiter='\t'))
calls = re.findall(r'^\s*([0-9a-f]+):\s+(?:bl|b)\s+0x129ff0 <ioctl@plt>$', text, re.M)
expected = [row['address'][2:] for row in rows]
assert calls == expected, (calls, expected)
end = text.index('dc74c:')
window = text[end - 260:end + 100]
for token in ('ldr\tw0, [x0]', 'mov\tw1, #0x5200',
              'sub\tx2, x29, #0x4', 'movk\tw1, #0x8004, lsl #16'):
    assert token in window, (token, window)
print('bionic-ioctl-facade: disassembly PASS callsites=1 request=0x80045200')
PY

"$android_cc" -std=c17 -Wall -Wextra -Werror -I"$dir/include" \
  -S -emit-llvm "$dir/probes/abi.c" -o "$tmp/android.ll"
grep -F '%struct.__va_list = type { ptr, ptr, ptr, i32, i32 }' \
  "$tmp/android.ll" >/dev/null || fail 'Android va_list layout'
sdk="$(xcrun --sdk macosx --show-sdk-path)"
host_cc="$(xcrun --find clang)"
host_cxx="$(xcrun --find clang++)"
"$host_cc" -arch arm64 -isysroot "$sdk" -std=c17 -I"$dir/include" \
  -S -emit-llvm "$dir/probes/abi.c" -o "$tmp/darwin.ll"
grep -F 'ret i64 8' "$tmp/darwin.ll" >/dev/null || fail 'Darwin va_list layout'

includes=(-I"$dir/include" -I"$root/tools/bionic-errno-tls/include")
cxxflags=(-arch arm64 -isysroot "$sdk" -std=c++20 -O2 -Wall -Wextra -Werror -Wpedantic)
"$host_cxx" "${cxxflags[@]}" "${includes[@]}" \
  -c "$dir/src/ioctl.cc" -o "$tmp/ioctl.o"
"$host_cc" -arch arm64 -isysroot "$sdk" \
  -c "$dir/src/aapcs64_entry.S" -o "$tmp/entry.o"
if nm -u "$tmp/ioctl.o" | awk '{print $NF}' |
    grep -E '^_(ioctl|dlsym|dlopen)$' >/dev/null; then
  fail 'host ioctl/dynamic fallback escaped'
fi
nm -u "$tmp/ioctl.o" | grep -F '_aim_bionic_errno_store' >/dev/null ||
  fail 'Bionic errno route missing'
for symbol in ioctl ioctl_resolve ioctl_activate ioctl_deactivate; do
  nm -gU "$tmp/ioctl.o" "$tmp/entry.o" |
    grep -F " _aim_bionic_$symbol" >/dev/null ||
    fail "definition: $symbol"
done
ar rcs "$tmp/libaim-bionic-ioctl.a" "$tmp/entry.o" "$tmp/ioctl.o"

fixture="$tmp/libbionic_ioctl_fixture.so"
"$android_cc" -std=c17 -O2 -fno-builtin -fPIC -fno-stack-protector \
  -U_FORTIFY_SOURCE -D_FORTIFY_SOURCE=0 -Wall -Wextra -Werror -shared -nostdlib \
  -fuse-ld=lld -Wl,--build-id=none -Wl,--hash-style=sysv -Wl,-z,now \
  -Wl,-z,norelro -Wl,-z,max-page-size=16384 \
  -Wl,-soname,libbionic_ioctl_fixture.so \
  -Wl,--version-script,"$dir/probes/exports.map" \
  "$dir/probes/fixture.c" -lc -o "$fixture"
"$readelf" --dyn-syms --wide "$fixture" |
  awk '$7=="UND"&&$8!=""{n=$8;sub(/@.*/,"",n);print n}' | sort -u \
  > "$tmp/fixture-imports"
printf '%s\n' ioctl > "$tmp/fixture-expected"
diff -u "$tmp/fixture-expected" "$tmp/fixture-imports" ||
  fail 'fixture import drift'
"$readelf" --dyn-syms --wide "$fixture" |
  awk '$7=="UND"&&$8!=""&&$8!="ioctl@LIBC"{bad=1}END{exit bad}' ||
  fail 'fixture version drift'

loader_target="$tmp/loader-target"
CARGO_TARGET_DIR="$loader_target" cargo build --quiet --release \
  --manifest-path "$root/crates/aim-elf-loader/Cargo.toml"
san=(-fsanitize=address,undefined -fno-omit-frame-pointer)
"$host_cxx" -arch arm64 -isysroot "$sdk" -std=c++20 -O1 -g \
  -Wall -Wextra -Werror -Wpedantic "${san[@]}" "${includes[@]}" \
  -c "$dir/src/ioctl.cc" -o "$tmp/ioctl-san.o"
"$host_cc" -arch arm64 -isysroot "$sdk" -std=c17 -O1 -g \
  -Wall -Wextra -Werror "${san[@]}" \
  -I"$root/tools/bionic-errno-tls/include" \
  -I"$root/tools/bionic-errno-tls/generated" \
  -c "$root/tools/bionic-errno-tls/src/errno_tls.c" -o "$tmp/errno-san.o"
"$host_cxx" -arch arm64 -isysroot "$sdk" -std=c++20 -O1 -g \
  -Wall -Wextra -Werror -Wpedantic "${san[@]}" "${includes[@]}" \
  -I"$root/crates/aim-elf-loader/include" \
  "$dir/probes/elf_runner.cc" "$dir/probes/socket_broker_stub.cc" \
  "$tmp/ioctl-san.o" "$tmp/entry.o" \
  "$tmp/errno-san.o" "$loader_target/release/libaim_elf_loader.a" \
  -framework Security -o "$tmp/elf-runner"
ASAN_OPTIONS=detect_leaks=0:halt_on_error=1 UBSAN_OPTIONS=halt_on_error=1 \
  "$tmp/elf-runner" "$fixture"

mkdir -p "$root/_build/bionic-ioctl-facade"
cp "$tmp/libaim-bionic-ioctl.a" "$root/_build/bionic-ioctl-facade/"
cp "$fixture" "$root/_build/bionic-ioctl-facade/"
clean
echo 'bionic-ioctl-facade: PASS import=1 callsites=1 request=RNDGETENTCNT AAPCS64=captured fd=virtual-kind-seam+quiescent errno=Bionic host-ioctl=0 ASan+UBSan'
