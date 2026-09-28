#!/bin/bash
set -euo pipefail
export LC_ALL=C

dir="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$dir/../.." && pwd)"
# shellcheck disable=SC1091
source "$dir/sources.lock"

fail() { echo "bionic-syscall-facade: $*" >&2; exit 3; }
missing() { echo "bionic-syscall-facade: missing $*" >&2; exit 2; }
sha() { shasum -a 256 "$1" | awk '{print $1}'; }
check() { [[ "$(sha "$1")" == "$2" ]] || fail "hash: $1"; }
clean() {
  git -C "$root" diff --check -- tools/bionic-syscall-facade || fail 'tracked diff check'
  git -C "$root" diff --cached --check -- tools/bionic-syscall-facade || fail 'staged diff check'
  while IFS= read -r -d '' file; do
    set +e
    whitespace="$(git -C "$root" diff --no-index --check /dev/null "$file" 2>&1)"
    status=$?
    set -e
    [[ -z "$whitespace" ]] || fail "untracked whitespace: $file: $whitespace"
    [[ $status -le 1 ]] || fail "could not diff-check: $file"
  done < <(git -C "$root" ls-files --others --exclude-standard -z -- tools/bionic-syscall-facade)
}

clean
tmp="$(mktemp -d "${TMPDIR:-/tmp}/bionic-syscall.XXXXXX")"
trap 'find "$tmp" -depth -delete' EXIT

check "$dir/README.md" "$README_SHA256"
check "$dir/audit.sh" "$AUDIT_SHA256"
check "$dir/include/aim_bionic_syscall.h" "$HEADER_SHA256"
check "$dir/manifests/imports.tsv" "$IMPORTS_SHA256"
check "$dir/manifests/callsites.tsv" "$CALLSITES_SHA256"
check "$dir/manifests/unsupported.tsv" "$UNSUPPORTED_SHA256"
check "$dir/upstream-sources.tsv" "$UPSTREAM_SOURCES_SHA256"
check "$dir/probes/abi.c" "$ABI_SHA256"
check "$dir/probes/elf_runner.cc" "$ELF_RUNNER_SHA256"
check "$dir/probes/exports.map" "$EXPORTS_SHA256"
check "$dir/probes/fixture.c" "$FIXTURE_SHA256"
check "$dir/src/aapcs64_entry.S" "$ENTRY_SHA256"
check "$dir/src/syscall.cc" "$PROVIDER_SHA256"

source_root="$root/_aosp/bionic-syscall-facade"
while IFS=$'\t' read -r relative size expected; do
  [[ "$relative" != path ]] || continue
  destination="$source_root/$relative"
  if [[ ! -f "$destination" ]]; then
    mkdir -p "$(dirname "$destination")"
    staged="$(mktemp "${TMPDIR:-/tmp}/bionic-syscall-source.XXXXXX")"
    curl -fsSL "https://android.googlesource.com/$LLVM_PROJECT/+/$LLVM_REVISION/$relative?format=TEXT" |
      base64 -D > "$staged"
    [[ "$(stat -f %z "$staged")" == "$size" && "$(sha "$staged")" == "$expected" ]] ||
      fail "download provenance: $relative"
    mv "$staged" "$destination"
  fi
  [[ "$(stat -f %z "$destination")" == "$size" && "$(sha "$destination")" == "$expected" ]] ||
    fail "source drift: $relative"
done < "$dir/upstream-sources.tsv"
printf '%s\n' "$LLVM_REVISION" > "$source_root/.source-revision"

python3 - "$source_root" <<'PY'
from pathlib import Path
import sys
root = Path(sys.argv[1])
atomic = (root / 'libcxx/src/atomic.cpp').read_text()
guard = (root / 'libcxxabi/src/cxa_guard_impl.h').read_text()
unwind = (root / 'libunwind/src/UnwindCursor.hpp').read_text()
assert 'syscall(SYS_futex, __ptr, FUTEX_WAIT_PRIVATE, __val, &__timeout, 0, 0);' in atomic
assert 'syscall(SYS_futex, __ptr, FUTEX_WAKE_PRIVATE, __notify_one ? 1 : INT_MAX, 0, 0, 0);' in atomic
assert 'static constexpr timespec __timeout = {2, 0};' in atomic
assert 'return static_cast<uint32_t>(syscall(SYS_gettid));' in guard
assert 'SYS_rt_sigprocmask, /*how=*/~0, sigsetAddr, nullptr, kernelSigsetSize' in unwind
assert 'errno == EFAULT || errno == EINVAL' in unwind
print('bionic-syscall-facade: source semantics PASS gettid+futex+readability')
PY

ndk="${ANDROID_NDK_ROOT:-${ANDROID_HOME:-$HOME/Library/Android/sdk}/ndk/$NDK_REVISION}"
tc="$ndk/toolchains/llvm/prebuilt/darwin-x86_64/bin"
android_cc="$tc/aarch64-linux-android${ANDROID_API}-clang"
readelf="$tc/llvm-readelf"
objdump="$tc/llvm-objdump"
libcxx="$tc/../sysroot/usr/lib/aarch64-linux-android/libc++_shared.so"
ndk_include="$tc/../sysroot/usr/include"
for input in "$android_cc" "$readelf" "$objdump" "$libcxx"; do
  [[ -e "$input" ]] || missing "$input"
done
check "$ndk/source.properties" "$NDK_SOURCE_PROPERTIES_SHA256"
[[ "$(stat -f %z "$libcxx")" == "$NDK_LIBCXX_SIZE" ]] || fail 'libc++ size drift'
check "$libcxx" "$NDK_LIBCXX_SHA256"
check "$ndk_include/asm-generic/unistd.h" "$NDK_UNISTD_SHA256"
check "$ndk_include/linux/futex.h" "$NDK_FUTEX_SHA256"
for definition in '__NR_futex 98' '__NR_getpid 172' \
                  '__NR_sched_setaffinity 122' \
                  '__NR_sched_getaffinity 123' '__NR_rt_sigprocmask 135' '__NR_gettid 178' \
                  '__NR_rt_tgsigqueueinfo 240' '__NR_getrandom 278'; do
  grep -F "$definition" "$ndk_include/asm-generic/unistd.h" >/dev/null ||
    fail "Android syscall number: $definition"
done
for definition in 'FUTEX_WAIT_PRIVATE (FUTEX_WAIT | FUTEX_PRIVATE_FLAG)' \
                  'FUTEX_WAKE_PRIVATE (FUTEX_WAKE | FUTEX_PRIVATE_FLAG)'; do
  grep -F "$definition" "$ndk_include/linux/futex.h" >/dev/null ||
    fail "Android futex operation: $definition"
done

master="$root/tools/bionic-libc-leaf-facade/imports/ndk-r28c-api35-arm64-libc.tsv"
check "$master" "$LIBC_IMPORT_MANIFEST_SHA256"
awk -F '\t' 'NR>1 && $1=="syscall"{print $1 "\t" $2 "\t" $3 "\t" $4}' "$master" \
  > "$tmp/master-demand"
tail -n +2 "$dir/manifests/imports.tsv" > "$tmp/locked-demand"
diff -u "$tmp/master-demand" "$tmp/locked-demand" || fail 'libc++ demand drift'
[[ "$(wc -l < "$tmp/master-demand" | tr -d ' ')" == 1 ]] || fail 'syscall demand count'
"$readelf" --dyn-syms --wide "$libcxx" |
  awk '$4=="FUNC"&&$5=="GLOBAL"&&$7=="UND"&&$8=="syscall@LIBC"{n++}END{exit n!=1}' ||
  fail 'real libc++ syscall import'

"$objdump" -d --no-show-raw-insn "$libcxx" > "$tmp/libcxx.disassembly"
python3 - "$tmp/libcxx.disassembly" "$dir/manifests/callsites.tsv" <<'PY'
import csv, re, sys
text = open(sys.argv[1]).read()
rows = list(csv.DictReader(open(sys.argv[2]), delimiter='\t'))
calls = re.findall(r'^\s*([0-9a-f]+):\s+(?:bl|b)\s+0x128d78 <syscall@plt>$', text, re.M)
expected = [row['address'][2:] for row in rows]
assert calls == expected, (calls, expected)
def window(address, before=420):
    marker = f'{address[2:]}:'
    end = text.index(marker)
    return text[max(0, end-before):end+100]
for address in ('0x9cc58', '0x9cca8'):
    assert 'mov\tw0, #0xb2' in window(address)
for address in ('0xd9f7c', '0xda02c', '0xda248'):
    part = window(address)
    assert 'mov\tw0, #0x62' in part and 'mov\tw2, #0x81' in part and 'mov\tw3, #0x7fffffff' in part
part = window('0xda1c8')
assert 'mov\tw0, #0x62' in part and 'mov\tw2, #0x81' in part and 'mov\tw3, #0x1' in part
for address in ('0xda130', '0xda2f4'):
    part = window(address)
    assert 'mov\tw0, #0x62' in part and 'mov\tw2, #0x80' in part and 'adr\tx4, 0x71240' in part
part = window('0x125bbc')
for token in ('mov\tw0, #0x87', 'mov\tw1, #-0x1', 'mov\tx3, xzr', 'mov\tw4, #0x8'):
    assert token in part
print('bionic-syscall-facade: disassembly PASS callsites=9 gettid=2 futex=6 rt_sigprocmask=1')
PY

"$android_cc" -std=c17 -Wall -Wextra -Werror -S -emit-llvm \
  "$dir/probes/abi.c" -o "$tmp/android.ll"
grep -F '%struct.__va_list = type { ptr, ptr, ptr, i32, i32 }' "$tmp/android.ll" >/dev/null ||
  fail 'Android va_list layout'
sdk="$(xcrun --sdk macosx --show-sdk-path)"
host_cc="$(xcrun --find clang)"
host_cxx="$(xcrun --find clang++)"
"$host_cc" -arch arm64 -isysroot "$sdk" -std=c17 -S -emit-llvm \
  "$dir/probes/abi.c" -o "$tmp/darwin.ll"
grep -F 'alloca ptr, align 8' "$tmp/darwin.ll" >/dev/null || fail 'Darwin va_list layout'

includes=(-I"$dir/include" -I"$root/tools/bionic-errno-tls/include")
cxxflags=(-arch arm64 -isysroot "$sdk" -std=c++20 -O2 -Wall -Wextra -Werror -Wpedantic)
"$host_cxx" "${cxxflags[@]}" "${includes[@]}" -c "$dir/src/syscall.cc" -o "$tmp/syscall.o"
"$host_cc" -arch arm64 -isysroot "$sdk" -c "$dir/src/aapcs64_entry.S" -o "$tmp/entry.o"
if nm -u "$tmp/syscall.o" | awk '{print $NF}' |
    grep -E '^_(syscall|dlsym|dlopen|sigprocmask|pthread_sigmask)$' >/dev/null; then
  fail 'host syscall/dynamic/signal fallback escaped'
fi
nm -u "$tmp/syscall.o" | grep -F '_aim_bionic_errno_store' >/dev/null ||
  fail 'Bionic errno route missing'
for symbol in syscall syscall_resolve syscall_waiter_count syscall_spurious_wake; do
  nm -gU "$tmp/syscall.o" "$tmp/entry.o" | grep -F " _aim_bionic_$symbol" >/dev/null ||
    fail "definition: $symbol"
done
ar rcs "$tmp/libaim-bionic-syscall.a" "$tmp/entry.o" "$tmp/syscall.o"

fixture="$tmp/libbionic_syscall_fixture.so"
"$android_cc" -std=c17 -O2 -fno-builtin -fPIC -fno-stack-protector \
  -U_FORTIFY_SOURCE -D_FORTIFY_SOURCE=0 -Wall -Wextra -Werror -shared -nostdlib \
  -fuse-ld=lld -Wl,--build-id=none -Wl,--hash-style=sysv -Wl,-z,now \
  -Wl,-z,norelro -Wl,-z,max-page-size=16384 \
  -Wl,-soname,libbionic_syscall_fixture.so -Wl,--version-script,"$dir/probes/exports.map" \
  "$dir/probes/fixture.c" -lc -o "$fixture"
"$readelf" --dyn-syms --wide "$fixture" |
  awk '$7=="UND"&&$8!=""{n=$8;sub(/@.*/,"",n);print n}' | sort -u > "$tmp/fixture-imports"
printf '%s\n' syscall > "$tmp/fixture-expected"
diff -u "$tmp/fixture-expected" "$tmp/fixture-imports" || fail 'fixture import drift'
"$readelf" --dyn-syms --wide "$fixture" |
  awk '$7=="UND"&&$8!=""&&$8!="syscall@LIBC"{bad=1}END{exit bad}' ||
  fail 'fixture version drift'

loader_target="$tmp/loader-target"
CARGO_TARGET_DIR="$loader_target" cargo build --quiet --release \
  --manifest-path "$root/crates/aim-elf-loader/Cargo.toml"
san=(-fsanitize=address,undefined -fno-omit-frame-pointer)
"$host_cxx" -arch arm64 -isysroot "$sdk" -std=c++20 -O1 -g \
  -Wall -Wextra -Werror -Wpedantic "${san[@]}" "${includes[@]}" \
  -c "$dir/src/syscall.cc" -o "$tmp/syscall-san.o"
"$host_cc" -arch arm64 -isysroot "$sdk" -std=c17 -O1 -g \
  -Wall -Wextra -Werror "${san[@]}" \
  -I"$root/tools/bionic-errno-tls/include" -I"$root/tools/bionic-errno-tls/generated" \
  -c "$root/tools/bionic-errno-tls/src/errno_tls.c" -o "$tmp/errno-san.o"
"$host_cxx" -arch arm64 -isysroot "$sdk" -std=c++20 -O1 -g \
  -Wall -Wextra -Werror -Wpedantic "${san[@]}" "${includes[@]}" \
  -I"$root/crates/aim-elf-loader/include" \
  "$dir/probes/elf_runner.cc" "$tmp/syscall-san.o" "$tmp/entry.o" \
  "$tmp/errno-san.o" "$loader_target/release/libaim_elf_loader.a" \
  -framework Security -o "$tmp/elf-runner"
ASAN_OPTIONS=detect_leaks=0:halt_on_error=1 UBSAN_OPTIONS=halt_on_error=1 \
  "$tmp/elf-runner" "$fixture"

mkdir -p "$root/_build/bionic-syscall-facade"
cp "$tmp/libaim-bionic-syscall.a" "$root/_build/bionic-syscall-facade/"
cp "$fixture" "$root/_build/bionic-syscall-facade/"
clean
echo 'bionic-syscall-facade: PASS import=1 callsites=9 nr=98+122+123+135+172+178+240+278 AAPCS64=captured getpid=host gettid=stable getrandom=host-csprng sched-affinity=host-topology+target-validation+restriction-ENOSYS futex=private-side-table+monotonic-timeout+capacity257 readability=mach-vm rt-tgsigqueueinfo=virtual-tid errno=Bionic host-syscall=0 ASan+UBSan'
