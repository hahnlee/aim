#!/bin/bash
set -euo pipefail
export LC_ALL=C
dir="$(cd "$(dirname "$0")"&&pwd)"; root="$(cd "$dir/../.."&&pwd)"
fail(){ echo "bionic-stdio-facade: $1" >&2;exit 2; }
# shellcheck disable=SC1090
source "$dir/sources.lock"
sha(){ shasum -a 256 "$1"|awk '{print $1}'; }; check(){ [[ "$(sha "$1")" == "$2" ]]||fail "hash: $1"; }
clean(){
 [[ -z "$(find "$dir" "$root/tools/bionic-stdio-wide-integration" -type d -name target -print -quit)" ]]||fail 'local target exists'
 git -C "$root" diff --check -- tools/bionic-stdio-facade tools/bionic-stdio-wide-integration||fail 'tracked diff check'
 git -C "$root" diff --cached --check -- tools/bionic-stdio-facade tools/bionic-stdio-wide-integration||fail 'staged diff check'
 while IFS= read -r -d '' file;do
  set +e
  whitespace="$(git -C "$root" diff --no-index --check /dev/null "$file" 2>&1)"
  status=$?
  set -e
  [[ -z "$whitespace" ]]||fail "untracked whitespace: $file: $whitespace"
  [[ $status -le 1 ]]||fail "could not diff-check untracked file: $file"
 done < <(git -C "$root" ls-files --others --exclude-standard -z -- tools/bionic-stdio-facade tools/bionic-stdio-wide-integration)
}
clean
imports="$root/tools/bionic-libc-leaf-facade/imports/ndk-r28c-api35-arm64-libc.tsv";check "$imports" "$LIBC_IMPORT_MANIFEST_SHA256"
manifest="$dir/manifests/imports.tsv"
awk -F '\t' 'NR==FNR{if(FNR>1){if($1 in declared)exit 2;declared[$1]=$2 FS $3;n++}next}$1 in declared{if(declared[$1]!=$2 FS $3)exit 3;seen[$1]++}END{if(n!=18)exit 4;for(s in declared)if(seen[s]!=1)exit 5}' "$manifest" "$imports"||fail 'stdio manifest/source classification'
awk -F '\t' '$1=="__sF"&&$2=="OBJECT"&&$3=="C"{f=1}END{exit !f}' "$imports"||fail 'import __sF OBJECT C'
for s in fclose fflush fileno fopen fputc fread fseek fseeko ftello fwrite getc ungetc;do awk -F '\t' -v s="$s" '$1==s&&$2=="FUNC"&&$3=="B"{f=1}END{exit !f}' "$imports"||fail "import $s FUNC B";done
for s in fprintf vfprintf fputwc getwc ungetwc;do awk -F '\t' -v s="$s" '$1==s&&$2=="FUNC"&&$3=="B"{f=1}END{exit !f}' "$imports"||fail "reject import $s FUNC B";done
wide="$root/tools/bionic-wide-stdio-facade"
wide_imports_sha="$(awk -F= '$1=="IMPORTS_SHA256"{print $2}' "$wide/sources.lock")"
[[ "$(sha "$wide/manifests/imports.tsv")" == "$wide_imports_sha" ]]||fail 'wide stdio manifest drift'
for s in fputwc getwc ungetwc;do awk -F '\t' -v s="$s" '$1==s&&$2=="FUNC"&&$3=="LIBC"{f=1}END{exit !f}' "$wide/manifests/imports.tsv"||fail "wide owner import $s";done
check "$root/tools/bionic-errno-tls/sources.lock" "$BIONIC_ERRNO_LOCK_SHA256";check "$root/tools/bionic-errno-tls/src/errno_tls.c" "$BIONIC_ERRNO_SOURCE_SHA256";check "$root/tools/bionic-errno-tls/generated/darwin_to_android.inc" "$BIONIC_ERRNO_MAPPING_SHA256"
ndk="${ANDROID_NDK_ROOT:-$HOME/Library/Android/sdk/ndk/$NDK_REVISION}";inc="$ndk/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include";tc="$ndk/toolchains/llvm/prebuilt/darwin-x86_64/bin";cc="$tc/aarch64-linux-android35-clang";re="$tc/llvm-readelf"
check "$ndk/source.properties" "$NDK_SOURCE_PROPERTIES_SHA256";check "$inc/stdio.h" "$NDK_STDIO_SHA256";check "$inc/bits/struct_file.h" "$NDK_STRUCT_FILE_SHA256"
tmp="$(mktemp -d "${TMPDIR:-/tmp}/bionic-stdio.XXXXXX")";trap 'find "$tmp" -depth -delete' EXIT
"$cc" -std=c17 -Wall -Wextra -Werror -Wpedantic -fsyntax-only "$dir/probes/abi.c"
clang -std=c17 -Wall -Wextra -Werror -I"$dir/include" \
  "$dir/src/fortify.c" "$dir/probes/fortify.c" -o "$tmp/fortify"
"$tmp/fortify"
clang -arch arm64 -isysroot "$(xcrun --sdk macosx --show-sdk-path)" -std=c17 -O2 -Wall -Wextra -Werror -Wpedantic -I"$dir/include" -c "$dir/src/shims.c" -o "$tmp/shims.o"
nm -u "$tmp/shims.o"|sed 's/^[[:space:]]*//'|sort >"$tmp/u";cat >"$tmp/e" <<'EOF'
___error
___stack_chk_fail
___stack_chk_guard
_darwin_art_bionic_atoi
_darwin_art_bionic___fread_chk
_darwin_art_bionic___fwrite_chk
_darwin_art_bionic_errno_load
_darwin_art_bionic_errno_store
_darwin_art_bionic_realloc
_darwin_art_bionic_stdio_clearerr_core
_darwin_art_bionic_stdio_fclose_core
_darwin_art_bionic_stdio_feof_core
_darwin_art_bionic_stdio_ferror_core
_darwin_art_bionic_stdio_fflush_core
_darwin_art_bionic_stdio_fileno_core
_darwin_art_bionic_stdio_fopen_core
_darwin_art_bionic_stdio_fputc_core
_darwin_art_bionic_stdio_fread_core
_darwin_art_bionic_stdio_fseek_core
_darwin_art_bionic_stdio_ftello_core
_darwin_art_bionic_stdio_fwrite_core
_darwin_art_bionic_stdio_getc_core
_darwin_art_bionic_stdio_ungetc_core
_pthread_mutex_init
_pthread_mutex_lock
_pthread_mutex_trylock
_pthread_mutex_unlock
_pthread_mutexattr_destroy
_pthread_mutexattr_init
_pthread_mutexattr_settype
_pthread_once
EOF
diff -u "$tmp/e" "$tmp/u"||fail 'shim dependency'
fixture="$tmp/f.so";"$cc" -std=c17 -O2 -fno-builtin -fPIC -fno-stack-protector -U_FORTIFY_SOURCE -D_FORTIFY_SOURCE=0 -Wall -Wextra -Werror -Wpedantic -shared -nostdlib -Wl,-z,now -Wl,-z,norelro -Wl,--hash-style=sysv "$dir/probes/fixture.c" -o "$fixture"
"$re" -h "$fixture"|grep -F 'Machine:                           AArch64'>/dev/null||fail machine;"$re" --dyn-syms --wide "$fixture">"$tmp/d"
awk '$7=="UND"&&$8!=""{print $8}' "$tmp/d"|sort -u >"$tmp/iu";cat >"$tmp/ie" <<'EOF'
__errno
__sF
fclose
ferror
fflush
fgetc
fgets
fileno
fopen
fputc
fputs
fputwc
fread
fseek
fseeko
ftell
ftello
fwrite
getc
getwc
setbuf
stderr
stdin
stdout
ungetc
ungetwc
EOF
diff -u "$tmp/ie" "$tmp/iu"||fail 'fixture imports'
for s in bionic_stdio_fixture_basic bionic_stdio_fixture_race_setup bionic_stdio_fixture_race_write bionic_stdio_fixture_race_close bionic_stdio_fixture_race_after bionic_stdio_fixture_wide_race_setup bionic_stdio_fixture_wide_race_get bionic_stdio_fixture_wide_race_seek bionic_stdio_fixture_wide_race_close bionic_stdio_fixture_wide_race_after;do awk -v s="$s" '$7!="UND"&&$8==s{f=1}END{exit !f}' "$tmp/d"||fail "fixture export $s";done
integration="$root/tools/bionic-stdio-wide-integration/Cargo.toml"
CARGO_TARGET_DIR="$tmp/normal" cargo run --quiet --manifest-path "$integration" -- "$fixture"
CARGO_TARGET_DIR="$tmp/normal" cargo clippy --quiet --all-targets --manifest-path "$integration" -- -D warnings
BIONIC_STDIO_C_SANITIZER=address CARGO_TARGET_DIR="$tmp/asan" cargo run --quiet --manifest-path "$integration" -- "$fixture"
UBSAN_OPTIONS=halt_on_error=1 BIONIC_STDIO_C_SANITIZER=undefined CARGO_TARGET_DIR="$tmp/ubsan" cargo run --quiet --manifest-path "$integration" -- "$fixture"
BIONIC_STDIO_C_SANITIZER=thread CARGO_TARGET_DIR="$tmp/tsan" cargo run --quiet --manifest-path "$integration" -- "$fixture"
cargo fmt --manifest-path "$dir/Cargo.toml" -- --check
cargo fmt --manifest-path "$integration" -- --check;clean
echo 'bionic-stdio-facade: PASS imports=18 FILE152 API23-stream-objects fopen-e fgetc+fgets fputs binary+wide lease close/seek/reset+reuse C-ASan C-UBSan C-TSan target-clean'
