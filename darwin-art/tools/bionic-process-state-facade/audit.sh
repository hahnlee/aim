#!/bin/bash
set -euo pipefail
export LC_ALL=C
script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/../.." && pwd)"
fail() { echo "bionic-process-state-facade: $1" >&2; exit 2; }
[[ -f "$script_dir/sources.lock" ]] || fail 'missing sources.lock'
# shellcheck disable=SC1090
source "$script_dir/sources.lock"
sha256() { shasum -a 256 "$1" | awk '{print $1}'; }
check_hash() { [[ "$(sha256 "$1")" == "$2" ]] || fail "hash mismatch: $1"; }
check_target_clean() {
  [[ -z "$(find "$script_dir" -type d -name target -print -quit)" ]] ||
    fail 'module-local target directory must be absent'
}
check_diff_clean() {
  git -C "$project_root" diff --check -- tools/bionic-process-state-facade ||
    fail 'tracked worktree whitespace error'
  git -C "$project_root" diff --cached --check -- tools/bionic-process-state-facade ||
    fail 'staged whitespace error'
  while IFS= read -r -d '' file; do
    set +e
    whitespace="$(git -C "$project_root" diff --no-index --check /dev/null "$file" 2>&1)"
    status=$?
    set -e
    [[ -z "$whitespace" ]] || fail "untracked whitespace error: $file: $whitespace"
    [[ $status -le 1 ]] || fail "could not diff-check untracked file: $file"
  done < <(git -C "$project_root" ls-files --others --exclude-standard -z -- \
           tools/bionic-process-state-facade)
}

check_target_clean
check_diff_clean
check_hash "$script_dir/upstream/get_device_api_level_inlines.h" "$BIONIC_DEVICE_API_HEADER_SHA256"

imports="$project_root/tools/bionic-libc-leaf-facade/imports/ndk-r28c-api35-arm64-libc.tsv"
check_hash "$imports" "$LIBC_IMPORT_MANIFEST_SHA256"
for tuple in '__system_property_get:D' 'getauxval:D' 'getenv:C'; do
  symbol="${tuple%%:*}"; class="${tuple##*:}"
  awk -F '\t' -v s="$symbol" -v c="$class" \
    '$1==s && $2=="FUNC" && $3==c {f=1} END{exit !f}' "$imports" ||
    fail "import classification drift: $symbol"
done
check_hash "$project_root/tools/bionic-errno-tls/sources.lock" "$BIONIC_ERRNO_LOCK_SHA256"
check_hash "$project_root/tools/bionic-errno-tls/src/errno_tls.c" "$BIONIC_ERRNO_SOURCE_SHA256"
check_hash "$project_root/tools/bionic-errno-tls/generated/darwin_to_android.inc" "$BIONIC_ERRNO_MAPPING_SHA256"

ndk_root="${ANDROID_NDK_ROOT:-$HOME/Library/Android/sdk/ndk/$NDK_REVISION}"
include="$ndk_root/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include"
toolchain="$ndk_root/toolchains/llvm/prebuilt/darwin-x86_64/bin"
android_cc="$toolchain/aarch64-linux-android35-clang"
readelf="$toolchain/llvm-readelf"
check_hash "$ndk_root/source.properties" "$NDK_SOURCE_PROPERTIES_SHA256"
check_hash "$include/sys/system_properties.h" "$NDK_SYSTEM_PROPERTIES_SHA256"
check_hash "$include/linux/auxvec.h" "$NDK_AUXV_SHA256"
check_hash "$include/aarch64-linux-android/asm/hwcap.h" "$NDK_ARM64_HWCAP_SHA256"
check_hash "$include/stdlib.h" "$NDK_STDLIB_SHA256"
check_hash "$include/sys/auxv.h" "$NDK_SYS_AUXV_SHA256"

temp_root="$(mktemp -d "${TMPDIR:-/tmp}/bionic-process-state.XXXXXX")"
cleanup() { [[ "$temp_root" == "${TMPDIR:-/tmp}"/bionic-process-state.* ]] && find "$temp_root" -depth -delete; }
trap cleanup EXIT

"$android_cc" -std=c17 -Wall -Wextra -Werror -Wpedantic -fsyntax-only "$script_dir/probes/abi.c"
host_cc="$(xcrun --find clang)"; sdk="$(xcrun --sdk macosx --show-sdk-path)"
"$host_cc" -arch arm64 -isysroot "$sdk" -std=c17 -Wall -Wextra -Werror -Wpedantic -I"$script_dir/include" \
  "$script_dir/src/device_api_level.c" "$script_dir/probes/device-api-level-test.c" \
  -o "$temp_root/device-api-level-test"
"$temp_root/device-api-level-test"
"$host_cc" -arch arm64 -isysroot "$sdk" -std=c17 -O2 -Wall -Wextra -Werror \
  -Wpedantic -I"$script_dir/include" -c "$script_dir/src/shims.c" -o "$temp_root/shims.o"
nm -u "$temp_root/shims.o" | sed 's/^[[:space:]]*//' | sort >"$temp_root/undefined"
cat >"$temp_root/expected-undefined" <<'EOF'
___chkstk_darwin
___error
___memcpy_chk
___stack_chk_fail
___stack_chk_guard
___stderrp
__exit
__tlv_bootstrap
_arc4random
_arc4random_buf
_bzero
_darwin_art_bionic_affinity_get
_darwin_art_bionic_affinity_set
_darwin_art_bionic_android_get_device_api_level
_darwin_art_bionic_environ
_darwin_art_bionic_errno_load
_darwin_art_bionic_errno_set_from_darwin
_darwin_art_bionic_errno_store
_darwin_art_bionic_longjmp
_darwin_art_bionic_process_getauxval_core
_darwin_art_bionic_process_getenv_core
_darwin_art_bionic_process_property_area_serial_core
_darwin_art_bionic_process_property_find_core
_darwin_art_bionic_process_property_foreach_core
_darwin_art_bionic_process_property_get_core
_darwin_art_bionic_process_property_read_callback_core
_darwin_art_bionic_process_property_serial_core
_darwin_art_bionic_process_property_wait_core
_darwin_art_bionic_process_state_read_credential_ids_core
_darwin_art_bionic_setjmp
_exit
_fork
_fprintf
_getegid
_getentropy
_getenv
_geteuid
_getgid
_gethostname
_getpagesize
_getpid
_getrusage
_getuid
_kill
_mach_task_self_
_mach_vm_read_overwrite
_mach_vm_region
_mach_vm_write
_nice
_pthread_getname_np
_pthread_self
_pthread_setname_np
_raise
_sched_get_priority_max
_sched_get_priority_min
_sched_yield
_setenv
_setuid
_sigaction
_sigaltstack
_sigpending
_sigsuspend
_sigwait
_snprintf
_strcmp
_strcpy
_strlen
_unsetenv
_waitpid
_write
EOF
diff -u "$temp_root/expected-undefined" "$temp_root/undefined" || fail 'host dependency drift'
definitions="$(nm -gU "$temp_root/shims.o")"
for symbol in __system_property_find __system_property_foreach __system_property_get \
              __system_property_read_callback getauxval getenv process_state_resolve; do
  grep -F " _darwin_art_bionic_$symbol" <<<"$definitions" >/dev/null ||
    fail "missing prefixed C ABI definition: $symbol"
done
if awk '$2 ~ /^[TDS]$/ {print $3}' <<<"$definitions" |
   grep -Ev '^_darwin_art_bionic_' >/dev/null; then
  fail 'unprefixed C ABI definition escaped facade'
fi
host_passthrough="$(rg -n 'getenv\(|getauxval\(|dlopen|dlsym|dyld|RTLD_|/Users' \
  "$script_dir/src" | grep -v 'darwin_art_bionic_' | \
  grep -vE 'getenv\("DARWIN_ART_DEBUG_(PROPERTIES|MEDIA_CODEC|FAULT_MAP|NATIVE_TRAP)"\)' || true)"
if [[ -n "$host_passthrough" ]]; then
  fail 'host global passthrough or dynamic fallback entered facade'
fi

fixture="$temp_root/libfixture.so"
"$android_cc" -std=c17 -O2 -fno-builtin -fPIC -fno-stack-protector \
  -U_FORTIFY_SOURCE -D_FORTIFY_SOURCE=0 -Wall -Wextra -Werror -Wpedantic \
  -shared -nostdlib -Wl,-soname,libfixture.so -Wl,-z,now -Wl,-z,norelro \
  -Wl,--hash-style=sysv "$script_dir/probes/fixture.c" -o "$fixture"
"$readelf" -h "$fixture" | grep -F 'Machine:                           AArch64' >/dev/null ||
  fail 'fixture is not Android AArch64 ELF'
"$readelf" --dyn-syms --wide "$fixture" >"$temp_root/dynsyms"
awk '$7=="UND" && $8!="" {print $8}' "$temp_root/dynsyms" | sort -u >"$temp_root/fixture-undefined"
cat >"$temp_root/expected-fixture-undefined" <<'EOF'
__errno
__system_property_get
basename
getauxval
getentropy
getenv
raise
sigaction
signal
EOF
diff -u "$temp_root/expected-fixture-undefined" "$temp_root/fixture-undefined" || fail 'Android import drift'
for symbol in bionic_process_fixture_basic bionic_process_fixture_concurrent \
              bionic_process_fixture_verify_pointers bionic_process_fixture_after_teardown \
              bionic_process_fixture_signal_legacy \
              bionic_process_fixture_sigaction_query \
              bionic_process_fixture_sigaction_context_recovery; do
  grep -E "GLOBAL DEFAULT +[0-9]+ $symbol\$" "$temp_root/dynsyms" >/dev/null ||
    fail "Android fixture export missing: $symbol"
done

CARGO_TARGET_DIR="$temp_root/target" cargo run --quiet --features standalone-syscall --manifest-path "$script_dir/Cargo.toml" -- "$fixture"
CARGO_TARGET_DIR="$temp_root/target" cargo clippy --quiet --features standalone-syscall --manifest-path "$script_dir/Cargo.toml" -- -D warnings
cargo fmt --manifest-path "$script_dir/Cargo.toml" -- --check
check_target_clean
check_diff_clean
echo 'bionic-process-state-facade: PASS AndroidELF imports=9 threads=8x1000 basename-TLS+getentropy-CSPRNG getrlimit-copyout-EFAULT stable-pointers teardown signal-trampoline ucontext-recovery target-clean diff-clean'
