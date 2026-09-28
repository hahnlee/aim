#!/bin/bash
set -euo pipefail
export LC_ALL=C
dir="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$dir/../.." && pwd)"
fail() { echo "bionic-vm-facade: $1" >&2; exit 2; }
# shellcheck disable=SC1090
source "$dir/sources.lock"
sha() { shasum -a 256 "$1" | awk '{print $1}'; }
check() { [[ "$(sha "$1")" == "$2" ]] || fail "hash: $1"; }
clean() {
  [[ -z "$(find "$dir" -type d -name target -print -quit)" ]] || fail 'local target exists'
  git -C "$root" diff --check -- tools/bionic-vm-facade || fail 'tracked diff check'
  git -C "$root" diff --cached --check -- tools/bionic-vm-facade || fail 'staged diff check'
  while IFS= read -r -d '' file; do
    set +e
    whitespace="$(git -C "$root" diff --no-index --check /dev/null "$file" 2>&1)"
    status=$?
    set -e
    [[ -z "$whitespace" ]] || fail "untracked whitespace: $file: $whitespace"
    [[ $status -le 1 ]] || fail "could not diff-check untracked file: $file"
  done < <(git -C "$root" ls-files --others --exclude-standard -z -- tools/bionic-vm-facade)
}

clean
imports="$root/tools/bionic-libc-leaf-facade/imports/ndk-r28c-api35-arm64-libc.tsv"
check "$imports" "$LIBC_IMPORT_MANIFEST_SHA256"
[[ "$(tail -n +2 "$dir/manifests/imports.tsv" | wc -l | tr -d ' ')" == 8 ]] || fail 'manifest count'
while IFS=$'\t' read -r symbol kind demand policy; do
  [[ "$symbol" == symbol ]] && continue
  [[ "$kind" == FUNC && "$demand" == absent && -n "$policy" ]] || fail "manifest row: $symbol"
  ! awk -F '\t' -v symbol="$symbol" '$1==symbol{found=1}END{exit !found}' "$imports" || fail "pinned libc++ demand drift: $symbol"
done < "$dir/manifests/imports.tsv"

ndk="${ANDROID_NDK_ROOT:-$HOME/Library/Android/sdk/ndk/$NDK_REVISION}"
inc="$ndk/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include"
tc="$ndk/toolchains/llvm/prebuilt/darwin-x86_64/bin"
cc="$tc/aarch64-linux-android35-clang"
readelf="$tc/llvm-readelf"
libc="$ndk/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/lib/aarch64-linux-android/35/libc.so"
sdk="$(xcrun --sdk macosx --show-sdk-path)"
check "$ndk/source.properties" "$NDK_SOURCE_PROPERTIES_SHA256"
check "$inc/sys/mman.h" "$NDK_SYS_MMAN_SHA256"
check "$inc/linux/mman.h" "$NDK_LINUX_MMAN_SHA256"
check "$inc/asm-generic/mman-common.h" "$NDK_MMAN_COMMON_SHA256"
check "$inc/asm-generic/mman.h" "$NDK_MMAN_ARCH_SHA256"
check "$inc/aarch64-linux-android/asm/mman.h" "$NDK_ARM64_MMAN_SHA256"
check "$sdk/usr/include/sys/mman.h" "$DARWIN_MMAN_SHA256"
check "$libc" "$NDK_API35_ARM64_LIBC_SHA256"
check "$root/tools/bionic-errno-tls/sources.lock" "$BIONIC_ERRNO_LOCK_SHA256"
check "$root/tools/bionic-errno-tls/src/errno_tls.c" "$BIONIC_ERRNO_SOURCE_SHA256"
check "$root/tools/bionic-errno-tls/generated/darwin_to_android.inc" "$BIONIC_ERRNO_MAPPING_SHA256"
for symbol in madvise mmap mmap64 mprotect mremap munmap; do
  "$readelf" --dyn-syms --wide "$libc" | awk -v wanted="$symbol@@LIBC" '$5=="GLOBAL"&&$4=="FUNC"&&$8==wanted{found=1}END{exit !found}' || fail "libc export: $symbol@@LIBC"
done

tmp="$(mktemp -d "${TMPDIR:-/tmp}/bionic-vm.XXXXXX")"
trap 'find "$tmp" -depth -delete' EXIT
"$cc" -std=c17 -Wall -Wextra -Werror -Wpedantic -fsyntax-only "$dir/probes/abi.c"
for source in shims host; do
  clang -arch arm64 -isysroot "$sdk" -std=c17 -O2 -Wall -Wextra -Werror -Wpedantic \
    -I"$dir/include" -c "$dir/src/$source.c" -o "$tmp/$source.o"
done
nm -u "$tmp/shims.o" | sed 's/^[[:space:]]*//' | sort > "$tmp/shims.actual"
cat > "$tmp/shims.expected" <<'EOF'
___error
_aim_bionic_errno_store
_aim_bionic_vm_madvise_core
_aim_bionic_vm_mmap_core
_aim_bionic_vm_mprotect_core
_aim_bionic_vm_mremap_core
_aim_bionic_vm_munmap_core
_aim_host_vm_page_size
_memset
EOF
diff -u "$tmp/shims.expected" "$tmp/shims.actual" || fail 'shim dependencies'
nm -u "$tmp/host.o" | sed 's/^[[:space:]]*//' | sort > "$tmp/host.actual"
cat > "$tmp/host.expected" <<'EOF'
___error
_close
_getpagesize
_madvise
_mmap
_mprotect
_munmap
_sys_icache_invalidate
EOF
diff -u "$tmp/host.expected" "$tmp/host.actual" || fail 'host dependencies'

fixture="$tmp/fixture.so"
"$cc" -std=c17 -O2 -fno-builtin -fPIC -fno-stack-protector \
  -U_FORTIFY_SOURCE -D_FORTIFY_SOURCE=0 -Wall -Wextra -Werror -shared \
  -nostartfiles -Wl,-z,now -Wl,-z,norelro -Wl,--hash-style=sysv \
  "$dir/probes/fixture.c" -o "$fixture"
"$readelf" -h "$fixture" | grep -F 'Machine:                           AArch64' >/dev/null || fail 'fixture machine'
"$readelf" --dyn-syms --wide "$fixture" > "$tmp/dynsyms"
awk '$7=="UND"&&$8!=""{print $8}' "$tmp/dynsyms" | sort -u > "$tmp/imports.actual"
cat > "$tmp/imports.expected" <<'EOF'
__errno@LIBC
madvise@LIBC
mincore@LIBC
mmap64@LIBC
mmap@LIBC
mprotect@LIBC
mremap@LIBC
munmap@LIBC
EOF
diff -u "$tmp/imports.expected" "$tmp/imports.actual" || fail 'fixture imports'
for symbol in bionic_vm_fixture_basic bionic_vm_fixture_race_protect bionic_vm_fixture_race_setup bionic_vm_fixture_race_unmap; do
  awk -v wanted="$symbol" '$7!="UND"&&$8==wanted{found=1}END{exit !found}' "$tmp/dynsyms" || fail "fixture export: $symbol"
done
"$readelf" -d "$fixture" | awk '/Shared library:/{gsub(/.*\[|\].*/,"");print}' | sort > "$tmp/needed.actual"
cat > "$tmp/needed.expected" <<'EOF'
libc.so
libdl.so
EOF
diff -u "$tmp/needed.expected" "$tmp/needed.actual" || fail 'fixture needed libraries'

CARGO_TARGET_DIR="$tmp/normal" cargo run --quiet --manifest-path "$dir/Cargo.toml" -- "$fixture"
CARGO_TARGET_DIR="$tmp/normal" cargo clippy --quiet --all-targets --manifest-path "$dir/Cargo.toml" -- -D warnings
BIONIC_VM_C_SANITIZER=address CARGO_TARGET_DIR="$tmp/asan" cargo run --quiet --manifest-path "$dir/Cargo.toml" -- "$fixture"
UBSAN_OPTIONS=halt_on_error=1 BIONIC_VM_C_SANITIZER=undefined CARGO_TARGET_DIR="$tmp/ubsan" cargo run --quiet --manifest-path "$dir/Cargo.toml" -- "$fixture"
cargo fmt --manifest-path "$dir/Cargo.toml" -- --check
clean
echo "bionic-vm-facade: PASS libc++-demand=0 imports=8@LIBC anon-private+shared remap-move RW-RX-exec DONTNEED-zero mincore-page-size race C-ASan C-UBSan target-clean"
