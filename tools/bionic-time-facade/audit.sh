#!/bin/bash
set -euo pipefail
export LC_ALL=C

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/../.." && pwd)"
lock="$script_dir/sources.lock"
fail() { echo "bionic-time-facade: $1" >&2; exit 2; }
[[ -f "$lock" ]] || fail 'missing sources.lock'
# shellcheck disable=SC1090
source "$lock"

sha256() { shasum -a 256 "$1" | awk '{print $1}'; }
check_hash() {
  [[ "$(sha256 "$1")" == "$2" ]] || fail "hash mismatch: $1"
}

check_hash "$project_root/upstream/android16-os-constants-values.tsv" \
  "$OS_CONSTANTS_VALUES_SHA256"
check_hash "$project_root/tools/bionic-libc-leaf-facade/imports/ndk-r28c-api35-arm64-libc.tsv" \
  "$LIBC_IMPORT_MANIFEST_SHA256"
check_hash "$project_root/tools/bionic-errno-tls/sources.lock" \
  "$BIONIC_ERRNO_LOCK_SHA256"
check_hash "$project_root/tools/bionic-errno-tls/src/errno_tls.c" \
  "$BIONIC_ERRNO_SOURCE_SHA256"
check_hash "$project_root/tools/bionic-errno-tls/generated/darwin_to_android.inc" \
  "$BIONIC_ERRNO_MAPPING_SHA256"

for symbol in clock_gettime nanosleep sysconf; do
  awk -F '\t' -v wanted="$symbol" \
    '$1==wanted && $2=="FUNC" && $3=="B" {found=1} END{exit !found}' \
    "$project_root/tools/bionic-libc-leaf-facade/imports/ndk-r28c-api35-arm64-libc.tsv" ||
    fail "libc import classification drift: $symbol"
done
for pair in _SC_PAGESIZE:39 _SC_PAGE_SIZE:40 _SC_NPROCESSORS_CONF:96 _SC_NPROCESSORS_ONLN:97 _SC_PHYS_PAGES:98 _SC_AVPHYS_PAGES:99; do
  name="${pair%%:*}"
  value="${pair##*:}"
  awk -F '\t' -v wanted="$name" -v expected="$value" \
    '$1==wanted && $2==expected {found=1} END{exit !found}' \
    "$project_root/upstream/android16-os-constants-values.tsv" ||
    fail "Android OsConstants drift: $name"
done

ndk_root="${ANDROID_NDK_ROOT:-$HOME/Library/Android/sdk/ndk/$NDK_REVISION}"
toolchain="$ndk_root/toolchains/llvm/prebuilt/darwin-x86_64/bin"
android_cc="$toolchain/aarch64-linux-android35-clang"
readelf="$toolchain/llvm-readelf"
[[ -x "$android_cc" && -x "$readelf" ]] || fail 'missing pinned NDK toolchain'
check_hash "$ndk_root/source.properties" "$NDK_SOURCE_PROPERTIES_SHA256"
check_hash "$ndk_root/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include/linux/time.h" \
  "$NDK_LINUX_TIME_SHA256"
check_hash "$ndk_root/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include/bits/sysconf.h" \
  "$NDK_SYSCONF_SHA256"
check_hash "$ndk_root/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include/time.h" \
  "$NDK_TIME_SHA256"
check_hash "$ndk_root/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include/unistd.h" \
  "$NDK_UNISTD_SHA256"

temp_root="$(mktemp -d "${TMPDIR:-/tmp}/bionic-time-facade.XXXXXX")"
cleanup() {
  if [[ -n "$temp_root" && "$temp_root" == "${TMPDIR:-/tmp}"/bionic-time-facade.* ]]; then
    find "$temp_root" -depth -delete
  fi
}
trap cleanup EXIT

"$android_cc" -std=c17 -Wall -Wextra -Werror -Wpedantic -fsyntax-only \
  "$script_dir/probes/abi.c"

host_cc="$(xcrun --find clang)"
sdk_root="$(xcrun --sdk macosx --show-sdk-path)"
host_flags=(-arch arm64 -isysroot "$sdk_root" -std=c17 -O2 -Wall -Wextra
            -Werror -Wpedantic -I"$script_dir/include")
"$host_cc" "${host_flags[@]}" -c "$script_dir/src/shims.c" \
  -o "$temp_root/shims.o"
nm -u "$temp_root/shims.o" | sed 's/^[[:space:]]*//' | sort \
  >"$temp_root/undefined"
cat >"$temp_root/expected-undefined" <<'EOF'
___error
___stderrp
___udivti3
_asctime_r
_clock
_clock_getres
_clock_gettime
_ctime
_aim_bionic_errno_set_from_darwin
_aim_bionic_errno_store
_daylight
_difftime
_fprintf
_getenv
_getpagesize
_gettimeofday
_gmtime
_gmtime_r
_localtime
_localtime_r
_mach_continuous_time
_mach_timebase_info
_mktime
_nanosleep
_pthread_once
_setitimer
_sigaction
_sleep
_strcmp
_sysconf
_time
_timegm
_timezone
_tzname
_tzset
_usleep
EOF
diff -u "$temp_root/expected-undefined" "$temp_root/undefined" ||
  fail 'shim dependency drift'
definitions="$(nm -gU "$temp_root/shims.o")"
for symbol in clock_gettime nanosleep sysconf time_resolve time_data_resolve time_capability_failed; do
  grep -F " _aim_bionic_$symbol" <<<"$definitions" >/dev/null ||
    fail "missing prefixed definition $symbol"
done
if awk '$2 ~ /^[TDS]$/ {print $3}' <<<"$definitions" |
   grep -Ev '^_aim_bionic_' >/dev/null; then
  fail 'unprefixed global definition escaped time facade'
fi
visibility="$(nm -m "$temp_root/shims.o")"
for symbol in test_arm_alarm test_finish_alarm test_force_boottime_overflow; do
  grep -F "private external _aim_bionic_time_$symbol" <<<"$visibility" >/dev/null ||
    fail "test signal helper escaped hidden visibility: $symbol"
done
if rg -n 'dlopen|dlsym|dyld|NSLookupSymbolInImage|RTLD_' \
   "$script_dir/src" "$script_dir/include" "$script_dir/manifests" >/dev/null; then
  fail 'dynamic host fallback entered time facade'
fi

fixture="$temp_root/libbionic_time_fixture.so"
"$android_cc" -std=c17 -O2 -fno-builtin -fPIC -fno-stack-protector \
  -U_FORTIFY_SOURCE -D_FORTIFY_SOURCE=0 -Wall -Wextra -Werror -Wpedantic \
  -shared -nostdlib -Wl,-soname,libbionic_time_fixture.so -Wl,-z,now \
  -Wl,-z,norelro -Wl,--hash-style=sysv "$script_dir/probes/fixture.c" \
  -o "$fixture"
"$readelf" -h "$fixture" | grep -F 'Machine:                           AArch64' >/dev/null ||
  fail 'fixture is not Android AArch64 ELF'
"$readelf" --dyn-syms --wide "$fixture" >"$temp_root/dynsyms"
awk '$7=="UND" && $8!="" {print $8}' "$temp_root/dynsyms" | sort -u \
  >"$temp_root/fixture-undefined"
cat >"$temp_root/expected-fixture-undefined" <<'EOF'
__errno
clock_gettime
gettimeofday
nanosleep
sysconf
EOF
diff -u "$temp_root/expected-fixture-undefined" "$temp_root/fixture-undefined" ||
  fail 'Android time ELF import namespace drift'
for symbol in bionic_time_fixture_basic bionic_time_fixture_configured_processors \
  bionic_time_fixture_online_processors bionic_time_fixture_interrupted; do
  grep -E "GLOBAL DEFAULT +[0-9]+ $symbol\$" "$temp_root/dynsyms" >/dev/null ||
    fail "fixture runner export missing: $symbol"
done

CARGO_TARGET_DIR="$temp_root/cargo-target" cargo run --quiet \
  --manifest-path "$script_dir/Cargo.toml" -- "$fixture"
CARGO_TARGET_DIR="$temp_root/cargo-target" cargo clippy --quiet \
  --manifest-path "$script_dir/Cargo.toml" -- -D warnings
cargo fmt --manifest-path "$script_dir/Cargo.toml" -- --check

echo 'bionic-time-facade: PASS AndroidELF imports=5 clocks=5 timeval=64-bit-usec nanosleep-EINTR sysconf=6 closed-resolver'
