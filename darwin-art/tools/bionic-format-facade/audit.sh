#!/bin/bash
set -euo pipefail
export LC_ALL=C

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"
tmp="$(mktemp -d "${TMPDIR:-/tmp}/bionic-format.XXXXXX")"
trap 'find "$tmp" -depth -delete' EXIT
fail(){ echo "bionic-format-facade: FAIL $*" >&2; exit 3; }
source "$here/sources.lock"
check(){ [[ "$(shasum -a 256 "$1"|awk '{print $1}')" == "$2" ]] || fail "source drift: $1"; }

ndk="${ANDROID_NDK_ROOT:-$HOME/Library/Android/sdk/ndk/28.2.13676358}"
check "$root/tools/bionic-libc-leaf-facade/imports/ndk-r28c-api35-arm64-libc.tsv" "$MASTER"
check "$here/aosp-corpus.tsv" "$AOSP"
check "$ndk/source.properties" "$NDK"
check "$here/include/darwin_art_bionic_format.h" "$HEADER"
check "$here/src/aapcs64_entry.S" "$ASM"
check "$here/src/format.cc" "$SOURCE"
check "$root/tools/bionic-libc-allocator-facade/src/allocator.c" "$ALLOCATOR_SOURCE_SHA256"
check "$root/tools/bionic-libc-allocator-facade/src/allocator_options.c" "$ALLOCATOR_OPTIONS_SOURCE_SHA256"
check "$here/probes/abi.c" "$ABI"
check "$here/probes/differential.cc" "$DIFF"
check "$here/probes/elf_runner.cc" "$RUNNER"
check "$here/probes/exports.map" "$EXPORTS"
check "$here/probes/fixture.c" "$FIXTURE"
check "$here/README.md" "$README"
check "$here/manifests/imports.tsv" "$IMPORTS"
android_cc="$ndk/toolchains/llvm/prebuilt/darwin-x86_64/bin/aarch64-linux-android35-clang"
readelf="$ndk/toolchains/llvm/prebuilt/darwin-x86_64/bin/llvm-readelf"
host_cc="$(xcrun --find clang)"
host_cxx="$(xcrun --find clang++)"
sdk="$(xcrun --sdk macosx --show-sdk-path)"
includes=(-I"$here/include" -I"$root/tools/bionic-libc-allocator-facade/include" -I"$root/tools/bionic-errno-tls/include")

"$android_cc" -std=c17 -Wall -Wextra -Werror -S -emit-llvm "$here/probes/abi.c" -o "$tmp/android.ll"
grep -F '%struct.__va_list = type { ptr, ptr, ptr, i32, i32 }' "$tmp/android.ll" >/dev/null || fail 'Android va_list layout drift'
grep -F 'getelementptr inbounds %struct.__va_list' "$tmp/android.ll" >/dev/null || fail 'Android va_arg cursor missing'
"$host_cc" -arch arm64 -isysroot "$sdk" -std=c17 -S -emit-llvm "$here/probes/abi.c" -o "$tmp/darwin.ll"
grep -F 'alloca ptr, align 8' "$tmp/darwin.ll" >/dev/null || fail 'Darwin va_list pointer layout drift'

build_objects(){
  local mode="$1" dir="$2"; mkdir -p "$dir"; local sanitize=()
  [[ "$mode" != san ]] || sanitize=(-fsanitize=address,undefined -fno-omit-frame-pointer)
  "$host_cxx" -arch arm64 -isysroot "$sdk" -std=c++20 -O2 -Wall -Wextra -Werror "${sanitize[@]+"${sanitize[@]}"}" "${includes[@]}" -c "$here/src/format.cc" -o "$dir/format.o"
  "$host_cc" -arch arm64 -isysroot "$sdk" -c "$here/src/aapcs64_entry.S" -o "$dir/entry.o"
  "$host_cc" -arch arm64 -isysroot "$sdk" -std=c17 -O2 -Wall -Wextra -Werror "${sanitize[@]+"${sanitize[@]}"}" -I"$root/tools/bionic-libc-allocator-facade/include" -c "$root/tools/bionic-libc-allocator-facade/src/allocator.c" -o "$dir/allocator.o"
  "$host_cc" -arch arm64 -isysroot "$sdk" -std=c17 -O2 -Wall -Wextra -Werror "${sanitize[@]+"${sanitize[@]}"}" -I"$root/tools/bionic-libc-allocator-facade/include" -c "$root/tools/bionic-libc-allocator-facade/src/allocator_options.c" -o "$dir/allocator_options.o"
  "$host_cc" -arch arm64 -isysroot "$sdk" -std=c17 -O2 -Wall -Wextra -Werror "${sanitize[@]+"${sanitize[@]}"}" -I"$root/tools/bionic-errno-tls/include" -I"$root/tools/bionic-errno-tls/generated" -c "$root/tools/bionic-errno-tls/src/errno_tls.c" -o "$dir/errno.o"
}
build_objects normal "$tmp/normal"
ar rcs "$tmp/libdarwin-art-bionic-format.a" "$tmp/normal"/*.o
for symbol in snprintf vsnprintf vasprintf format_resolve format_capability; do nm -gU "$tmp/libdarwin-art-bionic-format.a" | grep -F " _darwin_art_bionic_$symbol" >/dev/null || fail "missing $symbol"; done
nm -u "$tmp/normal/format.o" | grep -E ' _(v?snprintf|vasprintf|v?fprintf|v?sscanf)$' && fail 'host formatted libc dependency escaped'

build_objects san "$tmp/san"
"$host_cxx" -arch arm64 -isysroot "$sdk" -std=c++20 -O2 -Wall -Wextra -Werror -fsanitize=address,undefined -fno-omit-frame-pointer "${includes[@]}" "$here/probes/differential.cc" "$tmp/san"/*.o -o "$tmp/differential"
ASAN_OPTIONS=halt_on_error=1 UBSAN_OPTIONS=halt_on_error=1 "$tmp/differential"

fixture="$tmp/libbionic_format_fixture.so"
"$android_cc" -std=c17 -O2 -fno-builtin -fPIC -fno-stack-protector -U_FORTIFY_SOURCE -D_FORTIFY_SOURCE=0 -Wall -Wextra -Werror -shared -nostdlib -fuse-ld=lld -Wl,--build-id=none -Wl,--hash-style=sysv -Wl,-z,now -Wl,-z,norelro -Wl,-z,max-page-size=16384 -Wl,-soname,libbionic_format_fixture.so -Wl,--version-script,"$here/probes/exports.map" "$here/probes/fixture.c" -lc -o "$fixture"
"$readelf" --dyn-syms --wide "$fixture" | awk '$7=="UND"&&$8!=""{n=$8;sub(/@.*/,"",n);print n}' | sort -u > "$tmp/imports"
"$readelf" --dyn-syms --wide "$fixture" | awk '$7=="UND"&&$8!=""&&$8!~/@LIBC/{bad=1}END{exit bad}' || fail 'fixture import missing exact @LIBC'
printf '%s\n' __errno free snprintf vasprintf vsnprintf | sort > "$tmp/expected"
diff -u "$tmp/expected" "$tmp/imports" || fail 'fixture import drift'

CARGO_TARGET_DIR="$tmp/loader-target" cargo build --quiet --release --manifest-path "$root/crates/darwin-art-elf-loader/Cargo.toml"
"$host_cxx" -arch arm64 -isysroot "$sdk" -std=c++20 -O2 -Wall -Wextra -Werror "${includes[@]}" -I"$root/crates/darwin-art-elf-loader/include" "$here/probes/elf_runner.cc" "$tmp/normal"/*.o "$tmp/loader-target/release/libdarwin_art_elf_loader.a" -framework Security -o "$tmp/elf-runner"
"$tmp/elf-runner" "$fixture"

mkdir -p "$root/_build/bionic-format-facade"
cp "$tmp/libdarwin-art-bionic-format.a" "$root/_build/bionic-format-facade/"
cp "$fixture" "$root/_build/bionic-format-facade/"
echo 'bionic-format-facade: PASS support=snprintf+vsnprintf+vasprintf reject=FILE+scan+positional+%n+long-double host-format=0 ABI=AAPCS64/32-vs-Darwin/8'
