#!/bin/bash
set -euo pipefail
export LC_ALL=C

script_dir="$(cd "$(dirname "$0")" && pwd)"
manifest="$script_dir/imports/ndk-r28c-api35-arm64-allocators.tsv"
lock="$script_dir/sources.lock"
ndk_root="${ANDROID_NDK_ROOT:-$HOME/Library/Android/sdk/ndk/28.2.13676358}"
toolchain="$ndk_root/toolchains/llvm/prebuilt/darwin-x86_64/bin"
libcxx="$ndk_root/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/lib/aarch64-linux-android/libc++_shared.so"
readelf="$toolchain/llvm-readelf"
android_cc="$toolchain/aarch64-linux-android35-clang"

fail() { echo "bionic-libc-allocator-facade: $1" >&2; exit 2; }
[[ -f "$manifest" && -f "$lock" && -f "$libcxx" && -x "$readelf" && \
   -x "$android_cc" ]] || fail 'missing pinned input'
# shellcheck disable=SC1090
source "$lock"
[[ "$(stat -f '%z' "$libcxx")" == "$NDK_LIBCXX_SHARED_SIZE" ]] ||
  fail 'libc++ size mismatch'
[[ "$(shasum -a 256 "$libcxx" | awk '{print $1}')" == \
   "$NDK_LIBCXX_SHARED_SHA256" ]] || fail 'libc++ hash mismatch'

temp_root="$(mktemp -d "${TMPDIR:-/tmp}/bionic-libc-allocator.XXXXXX")"
cleanup() {
  [[ -n "$temp_root" && "$temp_root" == "${TMPDIR:-/tmp}"/bionic-libc-allocator.* ]] &&
    rm -rf "$temp_root"
}
trap cleanup EXIT

"$readelf" --dyn-syms --wide "$libcxx" |
  awk '$7=="UND" && $8 ~ /@LIBC/ {
    name=$8; sub(/@.*/,"",name);
    if (name=="free" || name=="malloc" || name=="posix_memalign" ||
        name=="realloc") print name "\t" $4
  }' | sort -u >"$temp_root/actual-imports"
tail -n +2 "$manifest" | cut -f1,2 >"$temp_root/expected-imports"
diff -u "$temp_root/expected-imports" "$temp_root/actual-imports" ||
  fail 'allocator import manifest drift'

cc="$(xcrun --find clang)"
ar="$(xcrun --find ar)"
sdk_root="$(xcrun --sdk macosx --show-sdk-path)"
flags=(-arch arm64 -isysroot "$sdk_root" -std=c17 -O2 -fno-builtin
       -Wall -Wextra -Werror -Wpedantic -I"$script_dir/include")
"$cc" "${flags[@]}" -fsyntax-only "$script_dir/probes/abi_signatures.c"
"$android_cc" -std=c17 -Wall -Wextra -Werror -Wpedantic \
  -I"$script_dir/include" -fsyntax-only "$script_dir/probes/abi_signatures.c"
"$cc" "${flags[@]}" -c "$script_dir/src/allocator.c" -o "$temp_root/allocator.o"
"$cc" "${flags[@]}" -c "$script_dir/src/allocator_options.c" -o "$temp_root/allocator_options.o"
"$ar" rcs "$temp_root/libdarwin-art-bionic-allocator.a" "$temp_root/allocator.o" "$temp_root/allocator_options.o"

nm -u "$temp_root/allocator.o" | sed 's/^[[:space:]]*//' | sort >"$temp_root/undefined"
cat >"$temp_root/expected-undefined" <<'EOF'
___error
_calloc
_darwin_art_bionic_errno_store
_darwin_art_bionic_mallopt
_free
_malloc
_malloc_size
_posix_memalign
_realloc
EOF
diff -u "$temp_root/expected-undefined" "$temp_root/undefined" ||
  fail 'Darwin allocator backend dependency drift'

definitions="$(nm -gU "$temp_root/allocator.o")"
for symbol in aligned_alloc free malloc malloc_result posix_memalign posix_memalign_result \
              realloc reallocarray realloc_result allocator_resolve allocator_table; do
  grep -F " _darwin_art_bionic_$symbol" <<<"$definitions" >/dev/null ||
    fail "missing prefixed definition $symbol"
done
if awk '$2 ~ /^[TDS]$/ {print $3}' <<<"$definitions" |
   grep -Ev '^_darwin_art_bionic_' >/dev/null; then
  fail 'unprefixed global definition escaped facade'
fi
if rg -n 'dlsym' "$script_dir/src" "$script_dir/include" \
   >/dev/null; then
  fail 'forbidden interposition or allocator scope expansion'
fi

"$cc" "${flags[@]}" "$script_dir/probes/differential.c" \
  "$temp_root/allocator.o" "$temp_root/allocator_options.o" -o "$temp_root/differential"
"$temp_root/differential"
"$cc" "${flags[@]}" -O1 -g -fsanitize=address,undefined \
  "$script_dir/probes/differential.c" "$script_dir/src/allocator.c" "$script_dir/src/allocator_options.c" \
  -o "$temp_root/differential-sanitized"
ASAN_OPTIONS=allocator_may_return_null=1 "$temp_root/differential-sanitized" >/dev/null

echo 'bionic-libc-allocator-facade: PASS imports=4 owner=Darwin errno=result-seam ASAN+UBSAN'
