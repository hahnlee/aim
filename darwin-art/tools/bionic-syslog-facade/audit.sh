#!/bin/bash
set -euo pipefail
export LC_ALL=C

dir="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$dir/../.." && pwd)"
# shellcheck disable=SC1091
source "$dir/sources.lock"

fail() { echo "bionic-syslog-facade: $*" >&2; exit 3; }
missing() { echo "bionic-syslog-facade: missing $*" >&2; exit 2; }
sha() { shasum -a 256 "$1" | awk '{print $1}'; }
check() { [[ "$(sha "$1")" == "$2" ]] || fail "hash: $1"; }
clean() {
  git -C "$root" diff --check -- tools/bionic-syslog-facade || fail 'tracked diff check'
  git -C "$root" diff --cached --check -- tools/bionic-syslog-facade || fail 'staged diff check'
  while IFS= read -r -d '' file; do
    set +e
    whitespace="$(git -C "$root" diff --no-index --check /dev/null "$file" 2>&1)"
    status=$?
    set -e
    [[ -z "$whitespace" ]] || fail "untracked whitespace: $file: $whitespace"
    [[ $status -le 1 ]] || fail "could not diff-check: $file"
  done < <(git -C "$root" ls-files --others --exclude-standard -z -- tools/bionic-syslog-facade)
}

clean
tmp="$(mktemp -d "${TMPDIR:-/tmp}/bionic-syslog.XXXXXX")"
trap 'find "$tmp" -depth -delete' EXIT

check "$dir/README.md" "$README_SHA256"
check "$dir/audit.sh" "$AUDIT_SHA256"
check "$dir/include/darwin_art_bionic_syslog.h" "$HEADER_SHA256"
check "$dir/manifests/imports.tsv" "$IMPORTS_SHA256"
check "$dir/manifests/extension-boundary.tsv" "$EXTENSION_BOUNDARY_SHA256"
check "$dir/probes/abi.c" "$ABI_SHA256"
check "$dir/probes/elf_runner.cc" "$ELF_RUNNER_SHA256"
check "$dir/probes/exports.map" "$EXPORTS_SHA256"
check "$dir/probes/fixture.c" "$FIXTURE_SHA256"
check "$dir/src/aapcs64_entry.S" "$ENTRY_SHA256"
check "$dir/src/syslog.cc" "$PROVIDER_SHA256"
check "$dir/upstream-sources.tsv" "$UPSTREAM_SOURCES_SHA256"

master="$root/tools/bionic-libc-leaf-facade/imports/ndk-r28c-api35-arm64-libc.tsv"
check "$master" "$LIBC_IMPORT_MANIFEST_SHA256"
awk -F '\t' 'NR>1 && ($1=="closelog" || $1=="openlog" || $1=="syslog") {
  print $1 "\t" $2 "\t" $3 "\t" $4
}' "$master" | sort > "$tmp/master-demand"
tail -n +2 "$dir/manifests/imports.tsv" | sort > "$tmp/locked-demand"
diff -u "$tmp/master-demand" "$tmp/locked-demand" || fail 'libc++ demand drift'
[[ "$(wc -l < "$tmp/master-demand" | tr -d ' ')" == 3 ]] || fail 'demand count'
[[ "$(tail -n +2 "$dir/manifests/extension-boundary.tsv" | cut -f1 | paste -sd, -)" == \
   'setlogmask,vsyslog' ]] || fail 'extension boundary drift'

source_root="$root/_aosp/bionic-syslog-facade"
while IFS=$'\t' read -r relative size expected; do
  [[ "$relative" != path ]] || continue
  destination="$source_root/$relative"
  if [[ ! -f "$destination" ]]; then
    mkdir -p "$(dirname "$destination")"
    staged="$(mktemp "${TMPDIR:-/tmp}/bionic-syslog-source.XXXXXX")"
    curl -fsSL "https://android.googlesource.com/$BIONIC_PROJECT/+/$BIONIC_REVISION/$relative?format=TEXT" |
      base64 -D > "$staged"
    [[ "$(stat -f %z "$staged")" == "$size" && "$(sha "$staged")" == "$expected" ]] ||
      fail "download provenance: $relative"
    mv "$staged" "$destination"
  fi
  [[ "$(stat -f %z "$destination")" == "$size" && "$(sha "$destination")" == "$expected" ]] ||
    fail "source drift: $relative"
done < "$dir/upstream-sources.tsv"
printf '%s\n' "$BIONIC_REVISION" > "$source_root/.source-revision"
python3 - "$source_root/libc/bionic/syslog.cpp" "$dir/src/syslog.cc" <<'PY'
from pathlib import Path
import sys
aosp = Path(sys.argv[1]).read_text()
provider = Path(sys.argv[2]).read_text()
assert 'static const char* syslog_log_tag = nullptr;' in aosp
assert 'static int syslog_priority_mask = 0xff;' in aosp
assert 'static int syslog_options = 0;' in aosp
assert 'syslog_log_tag = log_tag;' in aosp and 'syslog_options = options;' in aosp
assert 'priority &= LOG_PRIMASK;' in aosp
assert 'async_safe_format_log(android_log_priority, log_tag, "%s", log_line);' in aosp
assert '(syslog_options & LOG_PERROR) != 0' in aosp
assert 'dprintf(STDERR_FILENO, "%s: %s%s"' in aosp
assert '__android_log_write(AndroidPriority(priority), tag, line)' in provider
assert 'darwin_art_bionic_vsnprintf' in provider
assert 'syslog(' not in provider.replace('darwin_art_bionic_syslog', '')
assert 'getprogname' not in provider
assert 'g_default_tag' in provider and 'g_activated' in provider
print('bionic-syslog-facade: AOSP state+priority+PERROR semantics PASS')
PY

ndk="${ANDROID_NDK_ROOT:-$HOME/Library/Android/sdk/ndk/$NDK_REVISION}"
tc="$ndk/toolchains/llvm/prebuilt/darwin-x86_64/bin"
android_cc="$tc/aarch64-linux-android${ANDROID_API}-clang"
readelf="$tc/llvm-readelf"
objdump="$tc/llvm-objdump"
elf_nm="$tc/llvm-nm"
libc="$tc/../sysroot/usr/lib/aarch64-linux-android/$ANDROID_API/libc.so"
libcxx="$tc/../sysroot/usr/lib/aarch64-linux-android/libc++_shared.so"
ndk_include="$tc/../sysroot/usr/include"
for input in "$android_cc" "$readelf" "$objdump" "$elf_nm" "$libc" "$libcxx"; do
  [[ -e "$input" ]] || missing "$input"
done
check "$ndk/source.properties" "$NDK_SOURCE_PROPERTIES_SHA256"
check "$libc" "$NDK_LIBC_SHA256"
check "$libcxx" "$NDK_LIBCXX_SHA256"
check "$ndk_include/syslog.h" "$NDK_SYSLOG_HEADER_SHA256"
check "$ndk_include/android/log.h" "$NDK_ANDROID_LOG_HEADER_SHA256"
python3 - "$ndk_include/syslog.h" "$ndk_include/android/log.h" <<'PY'
from pathlib import Path
import sys
syslog = Path(sys.argv[1]).read_text()
android_log = Path(sys.argv[2]).read_text()
for definition in (
    '#define LOG_ERR 3', '#define LOG_WARNING 4', '#define LOG_INFO 6',
    '#define LOG_DEBUG 7', '#define LOG_PRIMASK 7', '#define LOG_PERROR 0x20',
):
    assert definition in syslog
for definition in (
    'ANDROID_LOG_DEBUG,', 'ANDROID_LOG_INFO,', 'ANDROID_LOG_WARN,',
    'ANDROID_LOG_ERROR,',
):
    assert definition in android_log
PY
for symbol in closelog openlog syslog; do
  "$readelf" --dyn-syms --wide "$libc" |
    awk -v wanted="$symbol@@LIBC" '$4=="FUNC"&&$5=="GLOBAL"&&$8==wanted{f=1}END{exit !f}' ||
    fail "API35 libc export: $symbol"
done
"$elf_nm" -anC "$libcxx" | grep -F 'std::__ndk1::__libcpp_verbose_abort(char const*, ...)' >/dev/null ||
  fail 'libc++ syslog owner missing'
"$objdump" -d --no-show-raw-insn --start-address=0xd9e00 --stop-address=0xd9ee0 "$libcxx" > "$tmp/owner-disassembly"
for call in 'openlog@plt' 'syslog@plt' 'closelog@plt'; do
  grep -F "$call" "$tmp/owner-disassembly" >/dev/null || fail "libc++ callsite: $call"
done
grep -E 'mov[[:space:]]+w0, #0x2' "$tmp/owner-disassembly" >/dev/null || fail 'libc++ priority drift'
"$readelf" -p .rodata "$libcxx" > "$tmp/libcxx-rodata"
grep -E '\[[[:space:]]*926\][[:space:]]+libc\+\+$' "$tmp/libcxx-rodata" >/dev/null || fail 'libc++ tag drift'
grep -E '\[[[:space:]]*1117\][[:space:]]+%s$' "$tmp/libcxx-rodata" >/dev/null || fail 'libc++ format drift'

"$android_cc" -std=c17 -Wall -Wextra -Werror -S -emit-llvm "$dir/probes/abi.c" -o "$tmp/android.ll"
grep -F '%struct.__va_list = type { ptr, ptr, ptr, i32, i32 }' "$tmp/android.ll" >/dev/null ||
  fail 'Android va_list layout'
sdk="$(xcrun --sdk macosx --show-sdk-path)"
host_cc="$(xcrun --find clang)"
host_cxx="$(xcrun --find clang++)"
"$host_cc" -arch arm64 -isysroot "$sdk" -std=c17 -S -emit-llvm "$dir/probes/abi.c" -o "$tmp/darwin.ll"
grep -F 'alloca ptr, align 8' "$tmp/darwin.ll" >/dev/null || fail 'Darwin va_list layout'

includes=(-I"$dir/include" -I"$root/tools/bionic-format-facade/include" \
  -I"$root/tools/bionic-libc-allocator-facade/include" -I"$root/tools/bionic-errno-tls/include")
san=(-fsanitize=address,undefined -fno-omit-frame-pointer)
check "$root/tools/bionic-format-facade/sources.lock" "$FORMAT_LOCK_SHA256"
check "$root/tools/bionic-format-facade/src/format.cc" "$FORMAT_PROVIDER_SHA256"
check "$root/tools/bionic-format-facade/src/aapcs64_entry.S" "$FORMAT_ENTRY_SHA256"
check "$root/tools/bionic-libc-allocator-facade/src/allocator.c" "$ALLOCATOR_PROVIDER_SHA256"
check "$root/tools/bionic-libc-allocator-facade/src/allocator_options.c" "$ALLOCATOR_OPTIONS_PROVIDER_SHA256"
check "$root/tools/bionic-errno-tls/src/errno_tls.c" "$ERRNO_PROVIDER_SHA256"
"$host_cxx" -arch arm64 -isysroot "$sdk" -std=c++20 -O1 -g -Wall -Wextra -Werror \
  "${san[@]}" "${includes[@]}" -c "$dir/src/syslog.cc" -o "$tmp/syslog.o"
"$host_cc" -arch arm64 -isysroot "$sdk" -c "$dir/src/aapcs64_entry.S" -o "$tmp/entry.o"
if nm -u "$tmp/syslog.o" | awk '{print $NF}' | grep -E '^_(openlog|closelog|syslog|vsyslog|getprogname|dlsym|dlopen)$' >/dev/null; then
  fail 'host syslog/dynamic fallback escaped'
fi
nm -u "$tmp/syslog.o" | grep -F '___android_log_write' >/dev/null || fail 'AOSP liblog route missing'
nm -u "$tmp/syslog.o" | grep -F '_darwin_art_bionic_vsnprintf' >/dev/null || fail 'Bionic formatter route missing'
ar rcs "$tmp/libdarwin-art-bionic-syslog.a" "$tmp/entry.o" "$tmp/syslog.o"
for symbol in closelog openlog syslog syslog_activate syslog_resolve; do
  nm -gU "$tmp/libdarwin-art-bionic-syslog.a" | grep -F " _darwin_art_bionic_$symbol" >/dev/null ||
    fail "definition: $symbol"
done

"$host_cxx" -arch arm64 -isysroot "$sdk" -std=c++20 -O1 -g -Wall -Wextra -Werror \
  "${san[@]}" "${includes[@]}" -c "$root/tools/bionic-format-facade/src/format.cc" -o "$tmp/format.o"
"$host_cc" -arch arm64 -isysroot "$sdk" \
  -c "$root/tools/bionic-format-facade/src/aapcs64_entry.S" -o "$tmp/format-entry.o"
"$host_cc" -arch arm64 -isysroot "$sdk" -std=c17 -O1 -g -Wall -Wextra -Werror \
  "${san[@]}" -I"$root/tools/bionic-libc-allocator-facade/include" \
  -c "$root/tools/bionic-libc-allocator-facade/src/allocator.c" -o "$tmp/allocator.o"
"$host_cc" -arch arm64 -isysroot "$sdk" -std=c17 -O1 -g -Wall -Wextra -Werror \
  "${san[@]}" -I"$root/tools/bionic-libc-allocator-facade/include" \
  -c "$root/tools/bionic-libc-allocator-facade/src/allocator_options.c" -o "$tmp/allocator_options.o"
"$host_cc" -arch arm64 -isysroot "$sdk" -std=c17 -O1 -g -Wall -Wextra -Werror \
  "${san[@]}" -I"$root/tools/bionic-errno-tls/include" -I"$root/tools/bionic-errno-tls/generated" \
  -c "$root/tools/bionic-errno-tls/src/errno_tls.c" -o "$tmp/errno.o"

fixture="$tmp/libbionic_syslog_fixture.so"
"$android_cc" -std=c17 -O2 -fno-builtin -fPIC -fno-stack-protector \
  -U_FORTIFY_SOURCE -D_FORTIFY_SOURCE=0 -Wall -Wextra -Werror -shared -nostdlib \
  -fuse-ld=lld -Wl,--build-id=none -Wl,--hash-style=sysv -Wl,-z,now \
  -Wl,-z,norelro -Wl,-z,max-page-size=16384 \
  -Wl,-soname,libbionic_syslog_fixture.so -Wl,--version-script,"$dir/probes/exports.map" \
  "$dir/probes/fixture.c" -lc -o "$fixture"
"$readelf" --dyn-syms --wide "$fixture" |
  awk '$7=="UND"&&$8!=""{n=$8;sub(/@.*/,"",n);print n}' | sort -u > "$tmp/fixture-imports"
printf '%s\n' closelog openlog syslog | sort > "$tmp/fixture-expected"
diff -u "$tmp/fixture-expected" "$tmp/fixture-imports" || fail 'fixture import drift'
"$readelf" --dyn-syms --wide "$fixture" |
  awk '$7=="UND"&&$8!=""&&$8!~/@LIBC/{bad=1}END{exit bad}' || fail 'fixture version drift'

loader_target="$tmp/loader-target"
CARGO_TARGET_DIR="$loader_target" cargo build --quiet --release \
  --manifest-path "$root/crates/darwin-art-elf-loader/Cargo.toml"
liblog="$root/_build/graphics-foundations/liblog-darwin.a"
[[ -f "$liblog" ]] || missing "$liblog (run build-android16-graphics-foundations.sh)"
check "$liblog" "$AOSP_LIBLOG_SHA256"
"$host_cxx" -arch arm64 -isysroot "$sdk" -std=c++20 -O1 -g -Wall -Wextra -Werror \
  "${san[@]}" "${includes[@]}" -I"$root/crates/darwin-art-elf-loader/include" \
  -I"$root/_aosp/system/logging/liblog/include" \
  "$dir/probes/elf_runner.cc" "$tmp/syslog.o" "$tmp/entry.o" "$tmp/format.o" \
  "$tmp/format-entry.o" \
  "$tmp/allocator.o" "$tmp/allocator_options.o" "$tmp/errno.o" "$loader_target/release/libdarwin_art_elf_loader.a" \
  "$liblog" -framework Security -o "$tmp/elf-runner"
set +e
ASAN_OPTIONS=detect_leaks=0:halt_on_error=1 UBSAN_OPTIONS=halt_on_error=1 \
  "$tmp/elf-runner" "$fixture"
runner_status=$?
set -e
[[ $runner_status == 0 ]] || fail "Android ELF runner status $runner_status"

mkdir -p "$root/_build/bionic-syslog-facade"
cp "$tmp/libdarwin-art-bionic-syslog.a" "$root/_build/bionic-syslog-facade/"
cp "$fixture" "$root/_build/bionic-syslog-facade/"
clean
echo 'bionic-syslog-facade: PASS imports=3 owner=libc++-verbose-abort/%s ABI=AAPCS64-captured liblog=AOSP priority+facility+state+PERROR errno=host-preserved host-syslog=0 ASan+UBSan'
