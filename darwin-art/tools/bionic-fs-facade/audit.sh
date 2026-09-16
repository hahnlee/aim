#!/bin/bash
set -euo pipefail
export LC_ALL=C

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/../.." && pwd)"
lock="$script_dir/sources.lock"
fail() { echo "bionic-fs-facade: $1" >&2; exit 2; }
[[ -f "$lock" ]] || fail 'missing sources.lock'
# shellcheck disable=SC1090
source "$lock"

sha256() { shasum -a 256 "$1" | awk '{print $1}'; }
check_hash() {
  [[ "$(sha256 "$1")" == "$2" ]] || fail "hash mismatch: $1"
}

check_hash "$project_root/crates/darwin-art-fs-broker/Cargo.toml" \
  "$FS_BROKER_CARGO_SHA256"
check_hash "$project_root/crates/darwin-art-fs-broker/src/lib.rs" \
  "$FS_BROKER_SOURCE_SHA256"
check_hash "$project_root/crates/darwin-art-fs-broker/src/inode_metadata.rs" \
  "$FS_BROKER_INODE_METADATA_SHA256"
check_hash "$project_root/crates/darwin-art-fs-broker/src/guest_path.rs" \
  "$FS_BROKER_GUEST_PATH_SHA256"
check_hash "$project_root/crates/darwin-art-fs-broker/src/guest_path/relative.rs" \
  "$FS_BROKER_RELATIVE_PATH_SHA256"
check_hash "$project_root/crates/darwin-art-fs-broker/src/guest_path/origin.rs" \
  "$FS_BROKER_MOUNT_ORIGIN_SHA256"
check_hash "$project_root/crates/darwin-art-fs-broker/src/guest_path/directory_name.rs" \
  "$FS_BROKER_DIRECTORY_NAME_SHA256"
check_hash "$project_root/crates/darwin-art-prefix/Cargo.toml" \
  "$PREFIX_CARGO_SHA256"
check_hash "$project_root/crates/darwin-art-prefix/src/lib.rs" \
  "$PREFIX_SOURCE_SHA256"
check_hash "$project_root/tools/bionic-errno-tls/sources.lock" \
  "$BIONIC_ERRNO_LOCK_SHA256"
check_hash "$project_root/tools/bionic-errno-tls/src/errno_tls.c" \
  "$BIONIC_ERRNO_SOURCE_SHA256"
check_hash "$project_root/tools/bionic-errno-tls/generated/darwin_to_android.inc" \
  "$BIONIC_ERRNO_MAPPING_SHA256"
check_hash "$project_root/upstream/android16-os-constants-values.tsv" \
  "$OS_CONSTANTS_VALUES_SHA256"
check_hash "$project_root/tools/bionic-libc-leaf-facade/imports/ndk-r28c-api35-arm64-libc.tsv" \
  "$LIBC_IMPORT_MANIFEST_SHA256"
check_hash "$project_root/tools/bionic-ioctl-facade/include/darwin_art_bionic_ioctl.h" \
  "$IOCTL_FACADE_HEADER_SHA256"
check_hash "$script_dir/manifests/synthetic-devices.tsv" \
  "$SYNTHETIC_DEVICES_SHA256"
diff -u <(printf '%s\n' \
  $'/dev/random\trandom-device\tO_RDONLY|O_CLOEXEC|O_NONBLOCK|O_LARGEFILE\tSecRandomCopyBytes\tS_IFCHR|0666\tmakedev(1,8)' \
  $'/dev/urandom\trandom-device\tO_RDONLY|O_CLOEXEC|O_NONBLOCK|O_LARGEFILE\tSecRandomCopyBytes\tS_IFCHR|0666\tmakedev(1,9)') \
  <(tail -n +2 "$script_dir/manifests/synthetic-devices.tsv") ||
  fail 'synthetic device policy drift'

random_source="$project_root/_aosp/bionic-fs-facade/libcxx/src/random.cpp"
if [[ ! -f "$random_source" ]]; then
  mkdir -p "$(dirname "$random_source")"
  curl -fsSL "https://android.googlesource.com/$LLVM_PROJECT/+/$LLVM_REVISION/libcxx/src/random.cpp?format=TEXT" |
    base64 -D > "$random_source"
fi
[[ "$(stat -f %z "$random_source")" == "$LLVM_RANDOM_SOURCE_SIZE" ]] ||
  fail 'LLVM random.cpp size drift'
check_hash "$random_source" "$LLVM_RANDOM_SOURCE_SHA256"
python3 - "$random_source" <<'PY'
import sys
text = open(sys.argv[1]).read()
assert 'random_device::random_device(const string& __token) : __f_(open(__token.c_str(), O_RDONLY))' in text
assert 'ssize_t s = read(__f_, p, n);' in text
assert '::ioctl(__f_, RNDGETENTCNT, &ent) < 0' in text
print('bionic-fs-facade: libc++ random_device source PASS open=token+O_RDONLY')
PY

symbols=(chdir close closedir fchmod fchmodat fdopendir fstat ftruncate getcwd
         isatty link lstat mkdir mkdirat open openat opendir pathconf read readdir readlink realpath
         remove rename stat statvfs symlink truncate unlinkat utimensat)
diff -u <(printf '%s\n' "${symbols[@]}") \
  <(tail -n +2 "$script_dir/manifests/imports.tsv" | cut -f1) ||
  fail 'filesystem facade import manifest drift'
for symbol in "${symbols[@]}"; do
  # mkdirat is an original libcutils import, not part of libc++'s demand
  # manifest. Validate its actual NDK libc export below instead of inventing
  # a libc++ classification for it.
  [[ "$symbol" != mkdirat ]] || continue
  awk -F '\t' -v wanted="$symbol" '$1==wanted && $2=="FUNC" && $3=="B" {found=1} END{exit !found}' \
    "$project_root/tools/bionic-libc-leaf-facade/imports/ndk-r28c-api35-arm64-libc.tsv" ||
    fail "libc import classification drift: $symbol"
done

ndk_root="${ANDROID_NDK_ROOT:-$HOME/Library/Android/sdk/ndk/$NDK_REVISION}"
toolchain="$ndk_root/toolchains/llvm/prebuilt/darwin-x86_64/bin"
android_cc="$toolchain/aarch64-linux-android35-clang"
readelf="$toolchain/llvm-readelf"
[[ -x "$android_cc" && -x "$readelf" ]] || fail 'missing pinned NDK toolchain'
check_hash "$ndk_root/source.properties" "$NDK_SOURCE_PROPERTIES_SHA256"
"$readelf" --dyn-syms --wide \
  "$toolchain/../sysroot/usr/lib/aarch64-linux-android/35/libc.so" |
  awk '$4=="FUNC" && $5=="GLOBAL" && $7!="UND" && $8=="mkdirat@@LIBC" {found=1} END{exit !found}' ||
  fail 'NDK mkdirat@LIBC export drift'
check_hash "$ndk_root/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include/aarch64-linux-android/asm/fcntl.h" \
  "$NDK_ANDROID_ARM64_FCNTL_SHA256"
check_hash "$ndk_root/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include/linux/fcntl.h" \
  "$NDK_LINUX_FCNTL_SHA256"
check_hash "$ndk_root/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include/sys/stat.h" \
  "$NDK_SYS_STAT_SHA256"
check_hash "$ndk_root/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include/sys/statvfs.h" \
  "$NDK_SYS_STATVFS_SHA256"
check_hash "$ndk_root/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include/dirent.h" \
  "$NDK_DIRENT_SHA256"
check_hash "$ndk_root/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include/unistd.h" \
  "$NDK_UNISTD_SHA256"
check_hash "$ndk_root/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include/stdlib.h" \
  "$NDK_STDLIB_SHA256"
check_hash "$ndk_root/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include/stdio.h" \
  "$NDK_STDIO_SHA256"

temp_root="$(mktemp -d "${TMPDIR:-/tmp}/bionic-fs-facade.XXXXXX")"
cleanup() {
  if [[ -n "$temp_root" && "$temp_root" == "${TMPDIR:-/tmp}"/bionic-fs-facade.* ]]; then
    find "$temp_root" -depth -delete
  fi
}
trap cleanup EXIT

"$android_cc" -std=c17 -Wall -Wextra -Werror -Wpedantic -fsyntax-only \
  "$script_dir/probes/stat_layout.c"
host_cc="$(xcrun --find clang)"
sdk_root="$(xcrun --sdk macosx --show-sdk-path)"
host_flags=(-arch arm64 -isysroot "$sdk_root" -std=c17 -O2 -Wall -Wextra
            -Werror -Wpedantic -I"$script_dir/include"
            -I"$project_root/tools/bionic-ioctl-facade/include")
"$host_cc" "${host_flags[@]}" "$script_dir/probes/fortify_io.c" \
  -o "$temp_root/fortify-io"
"$temp_root/fortify-io"
"$host_cc" "${host_flags[@]}" -c "$script_dir/src/shims.c" \
  -o "$temp_root/shims.o"
nm -u "$temp_root/shims.o" | sed 's/^[[:space:]]*//' | sort \
  >"$temp_root/undefined"
cat >"$temp_root/expected-undefined" <<'EOF'
___error
___stack_chk_fail
___stack_chk_guard
_abort
_bzero
_closedir
_darwin_art_bionic_errno_load
_darwin_art_bionic_errno_store
_darwin_art_bionic_fs_chdir_core
_darwin_art_bionic_fs_chmod_core
_darwin_art_bionic_fs_close_core
_darwin_art_bionic_fs_closedir_core
_darwin_art_bionic_fs_dirfd_core
_darwin_art_bionic_fs_fchdir_core
_darwin_art_bionic_fs_fchmod_core
_darwin_art_bionic_fs_fchmodat_core
_darwin_art_bionic_fs_fchown_core
_darwin_art_bionic_fs_fdopendir_core
_darwin_art_bionic_fs_flock_core
_darwin_art_bionic_fs_fstat_core
_darwin_art_bionic_fs_fstatat_core
_darwin_art_bionic_fs_fsync_core
_darwin_art_bionic_fs_ftruncate_core
_darwin_art_bionic_fs_getcwd_core
_darwin_art_bionic_fs_isatty_core
_darwin_art_bionic_fs_link_core
_darwin_art_bionic_fs_lseek_core
_darwin_art_bionic_fs_lstat_core
_darwin_art_bionic_fs_mkdir_core
_darwin_art_bionic_fs_mkdirat_core
_darwin_art_bionic_fs_open_core
_darwin_art_bionic_fs_openat_core
_darwin_art_bionic_fs_opendir_core
_darwin_art_bionic_fs_pathconf_core
_darwin_art_bionic_fs_posix_fallocate_core
_darwin_art_bionic_fs_pread_core
_darwin_art_bionic_fs_pwrite_core
_darwin_art_bionic_fs_read_core
_darwin_art_bionic_fs_readdir_core
_darwin_art_bionic_fs_readlink_core
_darwin_art_bionic_fs_readv_core
_darwin_art_bionic_fs_realpath_core
_darwin_art_bionic_fs_remove_core
_darwin_art_bionic_fs_rename_core
_darwin_art_bionic_fs_rewinddir_core
_darwin_art_bionic_fs_stat_core
_darwin_art_bionic_fs_statvfs_core
_darwin_art_bionic_fs_symlink_core
_darwin_art_bionic_fs_truncate_core
_darwin_art_bionic_fs_unlinkat_core
_darwin_art_bionic_fs_utimensat_core
_darwin_art_bionic_fs_write_core
_darwin_art_bionic_fs_writev_core
_fcntl
_fdopendir
_fpathconf
_fstatvfs
_getenv
_getpid
_mach_task_self_
_mach_vm_region_recurse
_memcpy
_proc_pidinfo
_readdir
_rewinddir
_strlen
_sysconf
_write
EOF
diff -u "$temp_root/expected-undefined" "$temp_root/undefined" ||
  fail 'shim dependency drift'
definitions="$(nm -gU "$temp_root/shims.o")"
for symbol in "${symbols[@]}" fs_resolve; do
  grep -F " _darwin_art_bionic_$symbol" <<<"$definitions" >/dev/null ||
    fail "missing prefixed definition $symbol"
done
for symbol in __read_chk __write_chk; do
  grep -F " _darwin_art_bionic_$symbol" <<<"$definitions" >/dev/null ||
    fail "missing fortified definition $symbol"
done
for symbol in chmod lseek lseek64 pread pwrite readv rewinddir write writev; do
  grep -F " _darwin_art_bionic_$symbol" <<<"$definitions" >/dev/null ||
    fail "missing extended prefixed definition $symbol"
done
if awk '$2 ~ /^[TDS]$/ {print $3}' <<<"$definitions" |
   grep -Ev '^_darwin_art_bionic_' >/dev/null; then
  fail 'unprefixed global definition escaped filesystem facade'
fi
visibility="$(nm -m "$temp_root/shims.o")"
for symbol in host_closedir host_fdopendir host_fpathconf host_fstatvfs host_readdir; do
  grep -F "private external _darwin_art_bionic_fs_$symbol" <<<"$visibility" >/dev/null ||
    fail "host DIR helper escaped hidden visibility: $symbol"
done
if rg -n 'dlopen|dlsym|dyld|NSLookupSymbolInImage|RTLD_|socket|(^|[^[:alnum:]_.])(sendfile|syscall)[[:space:]]*\(' \
   "$script_dir/src" "$script_dir/include" "$script_dir/manifests" >/dev/null; then
  fail 'forbidden resolver, host sendfile, or raw syscall entered facade'
fi

CARGO_TARGET_DIR="$temp_root/cargo-target" cargo test --quiet \
  --manifest-path "$project_root/crates/darwin-art-fs-broker/Cargo.toml"
CARGO_TARGET_DIR="$temp_root/cargo-target" cargo test --quiet \
  --manifest-path "$project_root/crates/darwin-art-prefix/Cargo.toml"
CARGO_TARGET_DIR="$temp_root/cargo-target" cargo test --quiet \
  --manifest-path "$script_dir/Cargo.toml"

fixture="$temp_root/libbionic_fs_fixture.so"
"$android_cc" -std=c17 -O2 -fno-builtin -fPIC -fno-stack-protector \
  -U_FORTIFY_SOURCE -D_FORTIFY_SOURCE=0 -Wall -Wextra -Werror -Wpedantic \
  -shared -nostdlib -Wl,-soname,libbionic_fs_fixture.so -Wl,-z,now \
  -Wl,-z,norelro -Wl,--hash-style=sysv "$script_dir/probes/fixture.c" \
  -o "$fixture"
"$readelf" -h "$fixture" | grep -F 'Machine:                           AArch64' >/dev/null ||
  fail 'fixture is not Android AArch64 ELF'
"$readelf" --dyn-syms --wide "$fixture" >"$temp_root/dynsyms"
awk '$7=="UND" && $8!="" {print $8}' "$temp_root/dynsyms" | sort -u \
  >"$temp_root/fixture-undefined"
cat >"$temp_root/expected-fixture-undefined" <<'EOF'
__errno
chdir
close
closedir
dirfd
fchmod
fchmodat
fdopendir
fstat
fstatat
ftruncate
getcwd
isatty
link
lstat
mkdir
mkdirat
open
openat
opendir
pathconf
read
readdir
readlink
realpath
remove
rename
stat
statvfs
symlink
truncate
unlinkat
utimensat
EOF
diff -u "$temp_root/expected-fixture-undefined" "$temp_root/fixture-undefined" ||
  fail 'Android filesystem ELF import namespace drift'
for export in bionic_fs_fixture_pc_2_symlinks bionic_fs_fixture_run \
  bionic_fs_fixture_statvfs_bsize bionic_fs_fixture_statvfs_flags; do
  grep -E "GLOBAL DEFAULT +[0-9]+ ${export}$" "$temp_root/dynsyms" >/dev/null ||
    fail "fixture export missing: $export"
done

mkdir -p "$temp_root/root/etc" "$temp_root/outside"
printf '%s' 'brokered-data' >"$temp_root/root/etc/payload.txt"
printf '%s' 'outside-secret' >"$temp_root/outside/secret"
ln -s "$temp_root/outside/secret" "$temp_root/root/etc/outside-link"
ln -s "$temp_root/outside" "$temp_root/root/etc/outside-dir-link"
CARGO_TARGET_DIR="$temp_root/cargo-target" cargo run --quiet \
  --manifest-path "$script_dir/Cargo.toml" -- "$fixture" "$temp_root/root"
nm -gU "$temp_root/cargo-target/debug/bionic-fs-facade" |
  grep -F ' _darwin_art_bionic_fs_ioctl_fd_lookup' >/dev/null ||
  fail 'ioctl fd lookup callback missing'
nm -gU "$temp_root/cargo-target/debug/bionic-fs-facade" |
  grep -F ' _darwin_art_bionic_fs_sendfile_transfer' >/dev/null ||
  fail 'sendfile transfer callback missing'
for symbol in process_install process_uninstall process_has_capability_failure seed_private_directory; do
  nm -gU "$temp_root/cargo-target/debug/bionic-fs-facade" |
    grep -F " _darwin_art_bionic_fs_${symbol}" >/dev/null ||
    fail "process owner lifecycle symbol missing: $symbol"
done
CARGO_TARGET_DIR="$temp_root/cargo-target" cargo clippy --quiet \
  --all-targets --manifest-path "$script_dir/Cargo.toml" -- -D warnings
ASAN_OPTIONS=detect_leaks=0:halt_on_error=1 BIONIC_FS_C_SANITIZER=address \
  CARGO_TARGET_DIR="$temp_root/asan-target" \
  cargo run --quiet --manifest-path "$script_dir/Cargo.toml" -- \
  "$fixture" "$temp_root/root"
UBSAN_OPTIONS=halt_on_error=1 BIONIC_FS_C_SANITIZER=undefined \
  CARGO_TARGET_DIR="$temp_root/ubsan-target" cargo run --quiet \
  --manifest-path "$script_dir/Cargo.toml" -- "$fixture" "$temp_root/root"
cargo fmt --manifest-path "$script_dir/Cargo.toml" -- --check

echo 'bionic-fs-facade: PASS AndroidELF libc-imports=32 errno=1 immutable-root+private-/data path+cwd+DIR+fdopendir+dirfd+fstatat-empty random=Security+typed-fd sendfile=virtual-copy owner=process-wide+quiescent stat128/dirent280/statvfs112 closed-resolver ASan+UBSan'
