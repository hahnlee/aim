#!/bin/bash
set -euo pipefail
export LC_ALL=C

script_dir="$(cd "$(dirname "$0")" && pwd)"
manifest="$script_dir/imports/ndk-r28c-api35-arm64-libc.tsv"
lock="$script_dir/sources.lock"
ndk_root="${ANDROID_NDK_ROOT:-$HOME/Library/Android/sdk/ndk/28.2.13676358}"
libcxx="$ndk_root/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/lib/aarch64-linux-android/libc++_shared.so"
readelf="$ndk_root/toolchains/llvm/prebuilt/darwin-x86_64/bin/llvm-readelf"

fail() { echo "bionic-libc-leaf-facade: $1" >&2; exit 2; }
[[ -f "$manifest" && -f "$lock" && -f "$libcxx" && -x "$readelf" ]] || fail 'missing pinned input'
# shellcheck disable=SC1090
source "$lock"
[[ "$(stat -f '%z' "$libcxx")" == "$NDK_LIBCXX_SHARED_SIZE" ]] || fail 'libc++ size mismatch'
[[ "$(shasum -a 256 "$libcxx" | awk '{print $1}')" == "$NDK_LIBCXX_SHARED_SHA256" ]] || fail 'libc++ hash mismatch'

temp_root="$(mktemp -d "${TMPDIR:-/tmp}/bionic-libc-leaf.XXXXXX")"
cleanup() {
  [[ -n "$temp_root" && "$temp_root" == "${TMPDIR:-/tmp}"/bionic-libc-leaf.* ]] &&
    rm -rf "$temp_root"
}
trap cleanup EXIT

"$readelf" --dyn-syms --wide "$libcxx" |
  awk '$7=="UND" && $8 ~ /@LIBC/ { name=$8; sub(/@.*/,"",name); print name "\t" $4 }' |
  sort -u >"$temp_root/actual"
tail -n +2 "$manifest" | cut -f1,2 >"$temp_root/manifest"
diff -u "$temp_root/manifest" "$temp_root/actual" || fail 'libc import manifest drift'

[[ "$(awk -F '\t' 'NR>1&&$2=="FUNC"{n++}END{print n+0}' "$manifest")" == 159 ]] || fail 'expected 159 function imports'
[[ "$(awk -F '\t' 'NR>1&&$2=="OBJECT"{n++}END{print n+0}' "$manifest")" == 1 ]] || fail 'expected one object import'
[[ "$(awk -F '\t' 'NR>1&&$3=="A"{n++}END{print n+0}' "$manifest")" == 11 ]] || fail 'expected eleven category-A bindings'
awk -F '\t' 'NR>1 && $3 !~ /^[ABCD]$/ {exit 1}' "$manifest" || fail 'invalid category'

cc="$(xcrun --find clang)"
ar="$(xcrun --find ar)"
sdk_root="$(xcrun --sdk macosx --show-sdk-path)"
flags=(-arch arm64 -isysroot "$sdk_root" -std=c17 -O2 -fno-builtin -Wall -Wextra -Werror -Wpedantic -I"$script_dir/include")
"$cc" "${flags[@]}" -c "$script_dir/src/leaf.c" -o "$temp_root/leaf.o"
nm -u "$temp_root/leaf.o" | sed 's/^[[:space:]]*//' | sort \
  > "$temp_root/undefined"
cat > "$temp_root/expected-undefined" <<'EOF'
__tlv_bootstrap
_darwin_art_bionic___stack_chk_fail
_darwin_art_bionic_free
_darwin_art_bionic_malloc
EOF
diff -u "$temp_root/expected-undefined" "$temp_root/undefined" ||
  fail 'facade dependency boundary drift'
"$ar" rcs "$temp_root/libdarwin-art-bionic-libc-leaf.a" "$temp_root/leaf.o"

definitions="$(nm -gU "$temp_root/leaf.o")"
for symbol in atoi atol bsearch isprint memchr memcmp memcpy memmove memset memset_explicit qsort strcasecmp strcat strchr \
              strcmp strcpy strcspn strlcat strlen strncat strncmp strncpy strpbrk \
              strrchr strspn strstr wcslen wmemchr wmemcmp wmemcpy wmemmove wmemset; do
  grep -F " _darwin_art_bionic_$symbol" <<<"$definitions" >/dev/null || fail "missing facade definition $symbol"
done
for symbol in __memcpy_chk __memmove_chk __memset_chk; do
  grep -F " _darwin_art_bionic_$symbol" <<<"$definitions" >/dev/null || fail "missing fortified definition $symbol"
done
if awk '$2 ~ /^[TDS]$/ {print $3}' <<<"$definitions" | grep -Ev '^_darwin_art_bionic_' >/dev/null; then
  fail 'unprefixed global definition escaped facade'
fi

"$cc" "${flags[@]}" "$script_dir/probes/differential.c" "$temp_root/leaf.o" -o "$temp_root/differential"
"$temp_root/differential"
"$cc" "${flags[@]}" -fsanitize=address,undefined "$script_dir/probes/differential.c" \
  "$script_dir/src/leaf.c" -o "$temp_root/differential-sanitized"
"$temp_root/differential-sanitized" >/dev/null
echo 'bionic-libc-leaf-facade: PASS functions=159 object=1 bindings=67 dependencies=allocator+stack-check+Darwin-TLV'
