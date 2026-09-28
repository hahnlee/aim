#!/bin/bash
set -euo pipefail
export LC_ALL=C

dir="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$dir/../.." && pwd)"
# shellcheck disable=SC1091
source "$dir/sources.lock"

fail() { echo "bionic-abort-facade: $*" >&2; exit 3; }
missing() { echo "bionic-abort-facade: missing $*" >&2; exit 2; }
sha() { shasum -a 256 "$1" | awk '{print $1}'; }
check() { [[ "$(sha "$1")" == "$2" ]] || fail "hash: $1"; }
clean() {
  [[ -z "$(find "$dir" -type d -name target -print -quit)" ]] || fail 'local target exists'
  git -C "$root" diff --check -- tools/bionic-abort-facade || fail 'tracked diff check'
  git -C "$root" diff --cached --check -- tools/bionic-abort-facade || fail 'staged diff check'
  while IFS= read -r -d '' file; do
    set +e
    whitespace="$(git -C "$root" diff --no-index --check /dev/null "$file" 2>&1)"
    status=$?
    set -e
    [[ -z "$whitespace" ]] || fail "untracked whitespace: $file: $whitespace"
    [[ $status -le 1 ]] || fail "could not diff-check untracked file: $file"
  done < <(git -C "$root" ls-files --others --exclude-standard -z -- tools/bionic-abort-facade)
}

clean
tmp="$(mktemp -d "${TMPDIR:-/tmp}/bionic-abort.XXXXXX")"
cleanup() {
  if [[ "$tmp" == "${TMPDIR:-/tmp}"/bionic-abort.* ]]; then
    find "$tmp" -depth -delete
  fi
}
trap cleanup EXIT

master="$root/tools/bionic-libc-leaf-facade/imports/ndk-r28c-api35-arm64-libc.tsv"
errno_root="$root/tools/bionic-errno-tls"
check "$master" "$LIBC_IMPORT_MANIFEST_SHA256"
check "$errno_root/sources.lock" "$BIONIC_ERRNO_LOCK_SHA256"
check "$errno_root/src/errno_tls.c" "$BIONIC_ERRNO_SOURCE_SHA256"
check "$errno_root/generated/darwin_to_android.inc" "$BIONIC_ERRNO_MAPPING_SHA256"
check "$dir/upstream-sources.tsv" "$UPSTREAM_SOURCES_SHA256"
check "$dir/manifests/imports.tsv" "$IMPORTS_SHA256"
check "$dir/include/aim_bionic_abort.h" "$HEADER_SHA256"
check "$dir/src/provider.c" "$PROVIDER_SHA256"
check "$dir/src/main.rs" "$MAIN_SHA256"
check "$dir/probes/fixture.c" "$FIXTURE_SHA256"
check "$dir/probes/abi.c" "$ABI_SHA256"
check "$dir/probes/differential.c" "$DIFFERENTIAL_SHA256"
check "$dir/probes/death_support.c" "$DEATH_SUPPORT_SHA256"
check "$dir/probes/exports.map" "$EXPORTS_SHA256"
check "$dir/build.rs" "$BUILD_RS_SHA256"
check "$dir/Cargo.toml" "$CARGO_TOML_SHA256"
check "$dir/README.md" "$README_SHA256"

awk -F '\t' 'NR>1 && ($1=="abort" || $1=="android_set_abort_message") {
  print $1 "\t" $2 "\t" $3
}' "$master" | sort > "$tmp/master-demand"
tail -n +2 "$dir/manifests/imports.tsv" | cut -f1-3 | sort > "$tmp/locked-demand"
diff -u "$tmp/master-demand" "$tmp/locked-demand" ||
  fail 'pinned libc++ abort demand drift'
[[ "$(wc -l < "$tmp/master-demand" | tr -d ' ')" == 2 ]] ||
  fail 'abort demand count drift'

source_root="$root/_aosp/bionic-abort-facade"
[[ "$(tail -n +2 "$dir/upstream-sources.tsv" | wc -l | tr -d ' ')" == "$BIONIC_SOURCE_COUNT" ]] ||
  fail 'upstream source count drift'
while IFS=$'\t' read -r relative size expected; do
  [[ "$relative" != path ]] || continue
  destination="$source_root/$relative"
  if [[ ! -f "$destination" ]]; then
    mkdir -p "$(dirname "$destination")"
    staged="$(mktemp "${TMPDIR:-/tmp}/bionic-abort-source.XXXXXX")"
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

python3 - "$source_root" "$dir/src/provider.c" <<'PY'
import sys
from pathlib import Path
r = Path(sys.argv[1])
abort = (r/'libc/bionic/abort.cpp').read_text()
message = (r/'libc/bionic/android_set_abort_message.cpp').read_text()
provider = Path(sys.argv[2]).read_text()
assert 'sigfillset64(&mask)' in abort and 'sigdelset64(&mask, SIGABRT)' in abort
assert abort.count('sigprocmask64(SIG_SETMASK, &mask, nullptr)') == 2
assert abort.count('inline_raise(SIGABRT)') == 2
assert 'sa_handler = SIG_DFL' in abort and 'sa_flags = SA_RESTART' in abort
assert '_exit(127)' in abort
assert 'struct magic_abort_msg_t' in message
assert 'offsetof(magic_abort_msg_t, msg) == 2 * sizeof(uint64_t)' in message
assert '0xb18e40886ac388f0ULL' in message and '0xc6dfba755a1de0b5ULL' in message
assert 'abort_msg_lock' in message and 'abort_msg != nullptr' in message
assert 'msg = "(null)"' in message
assert 'sizeof(magic_abort_msg_t) + strlen(msg) + 1' in message
assert 'MAP_ANON | MAP_PRIVATE' in message
assert 'PR_SET_VMA_ANON_NAME' in message and '"abort message"' in message
assert 'new_magic_abort_message->msg.size = size' in message
assert provider.count('pthread_sigmask(SIG_SETMASK, &mask, NULL)') == 2
assert provider.count('pthread_kill(pthread_self(), SIGABRT)') == 2
assert '_exit(127)' in provider
assert '__attribute__((optnone))' in provider
assert 'PR_SET_VMA' not in provider and 'prctl(' not in provider
print('bionic-abort-facade: upstream=PASS signal-sequence+magic-layout+first-wins prctl-vma-name=explicit-gap')
PY

ndk="${ANDROID_NDK_ROOT:-$HOME/Library/Android/sdk/ndk/$NDK_REVISION}"
tc="$ndk/toolchains/llvm/prebuilt/darwin-x86_64/bin"
android_cc="$tc/aarch64-linux-android${ANDROID_API}-clang"
readelf="$tc/llvm-readelf"
elf_nm="$tc/llvm-nm"
ndk_include="$ndk/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include"
ndk_stdlib="$ndk_include/stdlib.h"
ndk_abort_header="$ndk_include/android/set_abort_message.h"
ndk_signal="$ndk_include/asm-generic/signal.h"
libc="$ndk/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/lib/aarch64-linux-android/$ANDROID_API/libc.so"
sdk="$(xcrun --sdk macosx --show-sdk-path)"
darwin_signal="$sdk/usr/include/sys/signal.h"
for input in "$ndk/source.properties" "$ndk_stdlib" "$ndk_abort_header" \
             "$ndk_signal" "$libc" "$darwin_signal"; do
  [[ -f "$input" ]] || missing "$input"
done
check "$ndk/source.properties" "$NDK_SOURCE_PROPERTIES_SHA256"
check "$ndk_stdlib" "$NDK_STDLIB_H_SHA256"
check "$ndk_abort_header" "$NDK_ABORT_MESSAGE_H_SHA256"
check "$ndk_signal" "$NDK_SIGNAL_H_SHA256"
check "$libc" "$NDK_API35_ARM64_LIBC_SHA256"
check "$darwin_signal" "$DARWIN_SIGNAL_H_SHA256"
grep -E '^#define SIGABRT[[:space:]]+6([[:space:]]|$)' "$ndk_signal" >/dev/null ||
  fail 'Android arm64 raw SIGABRT value drift'
grep -E '^#define[[:space:]]+SIGABRT[[:space:]]+6([[:space:]]|$)' "$darwin_signal" >/dev/null ||
  fail 'Darwin raw SIGABRT value drift'
for symbol in abort android_set_abort_message; do
  "$readelf" --dyn-syms --wide "$libc" |
    awk -v wanted="$symbol@@LIBC" '$4=="FUNC"&&$5=="GLOBAL"&&$8==wanted{found=1}END{exit !found}' ||
    fail "API-35 libc export: $symbol@@LIBC"
done
"$android_cc" -std=c17 -Wall -Wextra -Werror -Wpedantic -fsyntax-only "$dir/probes/abi.c"

host_cc="$(xcrun --find clang)"
host_flags=(-arch arm64 -isysroot "$sdk" -std=c17 -O1 -g -Wall -Wextra -Werror -Wpedantic -fno-builtin)
"$host_cc" "${host_flags[@]}" -I"$dir/include" \
  -c "$dir/src/provider.c" -o "$tmp/provider.o"
"$host_cc" "${host_flags[@]}" -I"$errno_root/include" -I"$errno_root/generated" \
  -c "$errno_root/src/errno_tls.c" -o "$tmp/errno.o"
nm -u "$tmp/provider.o" | sed 's/^[[:space:]]*//' | sort > "$tmp/provider.undefined"
cat > "$tmp/provider.expected" <<'EOF'
___error
___stderrp
__exit
_aim_bionic_errno_set_from_darwin
_fprintf
_memcpy
_mmap
_pthread_kill
_pthread_mutex_lock
_pthread_mutex_unlock
_pthread_self
_pthread_sigmask
_sigaction
_strcmp
_strlen
_write
EOF
diff -u "$tmp/provider.expected" "$tmp/provider.undefined" ||
  fail 'provider host dependency drift'
definitions="$(nm -gU "$tmp/provider.o")"
for symbol in abort android_set_abort_message; do
  grep -F " _aim_bionic_$symbol" <<< "$definitions" >/dev/null ||
    fail "missing prefixed definition: $symbol"
done
! nm -u "$tmp/provider.o" | awk '{print $NF}' |
  grep -E '^_(abort|raise|kill|signal|prctl|dlsym|dlopen)$' >/dev/null ||
  fail 'host abort/signal wrapper or dynamic fallback escaped'
! rg -n 'dlsym|dlopen|dyld|RTLD_' "$dir/src/provider.c" >/dev/null ||
  fail 'dynamic/global fallback in provider'

fixture="$tmp/libbionic_abort_fixture.so"
"$android_cc" -std=c17 -O2 -fno-builtin -fPIC -fno-stack-protector \
  -U_FORTIFY_SOURCE -D_FORTIFY_SOURCE=0 -Wall -Wextra -Werror -Wpedantic \
  -shared -nostdlib -fuse-ld=lld -Wl,--build-id=none -Wl,--hash-style=sysv \
  -Wl,-z,now -Wl,-z,norelro -Wl,-z,max-page-size=16384 \
  -Wl,-soname,libbionic_abort_fixture.so \
  -Wl,--version-script,"$dir/probes/exports.map" \
  "$dir/probes/fixture.c" -lc -o "$fixture"
check "$fixture" "$FIXTURE_ELF_SHA256"
file "$fixture" | grep -F 'ELF 64-bit LSB shared object, ARM aarch64' >/dev/null ||
  fail 'fixture is not Android arm64 ELF'
"$readelf" --dyn-syms --wide "$fixture" |
  awk '$7=="UND"&&$8!=""{print $8}' | sort -u > "$tmp/fixture.imports"
cat > "$tmp/fixture.expected" <<'EOF'
abort@LIBC
android_set_abort_message@LIBC
EOF
diff -u "$tmp/fixture.expected" "$tmp/fixture.imports" ||
  fail 'Android ELF exact import namespace drift'
[[ "$("$elf_nm" -D --defined-only "$fixture" | awk '$2=="T"{n++}END{print n+0}')" == 2 ]] ||
  fail 'fixture export count drift'

"$host_cc" "${host_flags[@]}" -I"$dir/include" "$dir/probes/differential.c" \
  "$tmp/provider.o" "$tmp/errno.o" -o "$tmp/differential"
differential_output="$("$tmp/differential")"
grep -F 'PASS message=first-wins+null+magic+size threads=8 death=default+blocked+ignored+returning host-errno=preserved' \
  <<< "$differential_output" >/dev/null || fail 'message/death differential gate failed'

san=(-fsanitize=address,undefined -fno-omit-frame-pointer)
"$host_cc" "${host_flags[@]}" "${san[@]}" -I"$dir/include" \
  -c "$dir/src/provider.c" -o "$tmp/provider-san.o"
"$host_cc" "${host_flags[@]}" "${san[@]}" -I"$errno_root/include" \
  -I"$errno_root/generated" -c "$errno_root/src/errno_tls.c" -o "$tmp/errno-san.o"
"$host_cc" "${host_flags[@]}" "${san[@]}" -I"$dir/include" \
  "$dir/probes/differential.c" "$tmp/provider-san.o" "$tmp/errno-san.o" \
  -o "$tmp/differential-san"
sanitizer_output="$(ASAN_OPTIONS=halt_on_error=1:detect_leaks=0 \
  UBSAN_OPTIONS=halt_on_error=1 "$tmp/differential-san" --messages-only)"
grep -F 'death=skipped-for-sanitizer' <<< "$sanitizer_output" >/dev/null ||
  fail 'ASan/UBSan message boundary gate failed'

CARGO_TARGET_DIR="$tmp/cargo-target" cargo run --quiet \
  --manifest-path "$dir/Cargo.toml" -- "$fixture"
CARGO_TARGET_DIR="$tmp/cargo-target" cargo clippy --quiet --all-targets \
  --manifest-path "$dir/Cargo.toml" -- -D warnings
cargo fmt --manifest-path "$dir/Cargo.toml" -- --check
clean

build_dir="$root/_build/bionic-abort-facade"
mkdir -p "$build_dir"
ar rcs "$build_dir/libaim-bionic-abort.a" "$tmp/provider.o" "$tmp/errno.o"
cp "$fixture" "$build_dir/"
printf '%s\n' "$differential_output"
printf '%s\n' "$sanitizer_output"
echo 'bionic-abort-facade: PASS libc++=2 AndroidELF=2@LIBC message=magic+first-wins+threads8 abort=4-death-modes SIGABRT=6 ASan+UBSan prctl-vma-name=gap target-clean'
