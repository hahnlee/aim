#!/bin/bash
set -euo pipefail
export LC_ALL=C
root="$(cd "$(dirname "$0")/.." && pwd)"
revision=09a271af557444c9a6b3f3146d6d474156fd6cdb
source_dir="$root/_aosp/android16-ftw"
out="$root/_build/android16-ftw"
ndk="${ANDROID_NDK_ROOT:-$HOME/Library/Android/sdk/ndk/28.2.13676358}"
toolchain="$ndk/toolchains/llvm/prebuilt/darwin-x86_64"
[[ "$(shasum -a 256 "$ndk/source.properties" | awk '{print $1}')" == \
  c00aa236fdb205e9be9edd9e2169763e48aca52735efff4e16f34205d49783b5 ]] || {
  echo 'ftw: NDK revision mismatch' >&2; exit 1;
}
mkdir -p "$source_dir/include" "$out"
while IFS=$'\t' read -r local_path upstream sha; do
  [[ "$local_path" != local ]] || continue
  if [[ ! -f "$source_dir/$local_path" ]]; then
    curl -fsSL "https://android.googlesource.com/platform/bionic/+/$revision/$upstream?format=TEXT" |
      base64 -D > "$source_dir/$local_path"
  fi
  [[ "$(shasum -a 256 "$source_dir/$local_path" | awk '{print $1}')" == "$sha" ]] || {
    echo "ftw: source hash mismatch: $local_path" >&2; exit 1;
  }
done < "$root/tools/android16-ftw/sources.tsv"

# Android layouts, native Darwin PCS. Never let unresolved Android libc calls
# bind accidentally to same-named Darwin libc functions when the archive links.
flags=(-target arm64-apple-macos14 -O2 -fno-builtin -fno-stack-protector
  -D__ANDROID_API__=36 '-D__LIBC_HIDDEN__=__attribute__((visibility("hidden")))'
  -nostdinc -isystem "$toolchain/lib/clang/19/include"
  -I"$source_dir/include"
  -isystem "$toolchain/sysroot/usr/include/aarch64-linux-android"
  -isystem "$toolchain/sysroot/usr/include"
  -include "$source_dir/include/openbsd-compat.h"
  -include "$root/tools/android16-ftw/ndk_declarations.h")
for symbol in __errno access calloc close closedir dirfd fchdir free fstat fstatat \
  getpagesize malloc memcpy memmove memset memset_explicit open opendir qsort \
  readdir reallocarray strlen strrchr; do
  flags+=("-D$symbol=darwin_art_ftw_import_$symbol")
done
for symbol in ftw nftw __fts_open fts_open fts_read fts_close fts_children fts_set recallocarray; do
  flags+=("-D$symbol=darwin_art_aosp_$symbol")
done
for source in fts.c recallocarray.c; do
  xcrun clang "${flags[@]}" -std=c17 -c "$source_dir/$source" -o "$out/$source.o"
done
xcrun clang++ "${flags[@]}" -std=c++17 -fno-exceptions -fno-rtti \
  -c "$source_dir/ftw.cpp" -o "$out/ftw.cpp.o"
xcrun ar rcs "$out/libandroid-ftw.a" "$out/fts.c.o" "$out/recallocarray.c.o" "$out/ftw.cpp.o"
nm -u "$out/libandroid-ftw.a" | awk '/^_/ {print $1}' | sort -u > "$out/undefined.txt"
if grep -Ev '^_darwin_art_(ftw_import_|aosp_)' "$out/undefined.txt"; then
  echo 'ftw: unclassified host import' >&2; exit 1
fi
binding_flags=(-target arm64-apple-macos14 -O2 -std=c17 -fno-builtin
  -Wall -Wextra -Werror -Wpedantic)
for provider in bionic-fs-facade bionic-ioctl-facade bionic-libc-allocator-facade \
  bionic-libc-leaf-facade bionic-errno-tls bionic-process-state-facade; do
  binding_flags+=("-I$root/tools/$provider/include")
done
xcrun clang "${binding_flags[@]}" -c "$root/tools/android16-ftw/bindings.c" \
  -o "$out/bindings.o"
xcrun clang "${binding_flags[@]}" -c "$root/tools/android16-ftw/resolver.c" \
  -o "$out/resolver.o"
# Keep upstream archive independent; the composed provider links both objects.
nm -gU "$out/bindings.o" | awk '$2 == "T" {print $3}' | sort -u > "$out/bindings.txt"
grep '^_darwin_art_ftw_import_' "$out/undefined.txt" > "$out/required-bindings.txt"
diff -u "$out/required-bindings.txt" "$out/bindings.txt"
echo "ftw: original AOSP archive and explicit guest bindings compiled: $out/libandroid-ftw.a"
