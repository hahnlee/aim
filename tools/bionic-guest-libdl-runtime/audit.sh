#!/bin/bash
set -euo pipefail
export LC_ALL=C

dir="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$dir/../.." && pwd)"
# shellcheck disable=SC1091
source "$dir/sources.lock"
fail() { echo "bionic-guest-libdl-runtime: $*" >&2; exit 3; }
missing() { echo "bionic-guest-libdl-runtime: missing $*" >&2; exit 2; }
source_tree_sha() {
  while IFS= read -r file; do
    printf '%s  %s\n' "$(shasum -a 256 "$file" | awk '{print $1}')" "${file#"$dir/"}"
  done < <(find "$dir" -type f ! -name sources.lock | sort)
}

git -C "$root" diff --check -- tools/bionic-guest-libdl-runtime || fail 'diff check'
[[ ! -e "$dir/target" ]] || fail 'source-local target directory'
[[ "$(source_tree_sha | shasum -a 256 | awk '{print $1}')" == \
  "$SOURCE_TREE_SHA256" ]] || fail 'source tree drift'
[[ "$(git -C "$root" rev-parse HEAD:crates/aim-elf-loader)" == \
  "$ELF_LOADER_TREE" ]] || fail 'ELF loader tree drift'
[[ "$(git -C "$root" rev-parse HEAD:tools/android-dso-namespace)" == \
  "$DSO_NAMESPACE_TREE" ]] || fail 'libdl facade tree drift'
[[ "$(git -C "$root" rev-parse HEAD:tools/android-classloader-native-state)" == \
  "$CLASSLOADER_STATE_TREE" ]] || fail 'ClassLoader state tree drift'

ndk_revision=28.2.13676358
android_api=35
ndk="${ANDROID_NDK_ROOT:-${ANDROID_HOME:-$HOME/Library/Android/sdk}/ndk/$ndk_revision}"
tc="$ndk/toolchains/llvm/prebuilt/darwin-x86_64/bin"
android_cc="$tc/aarch64-linux-android${android_api}-clang"
readelf="$tc/llvm-readelf"
for input in "$android_cc" "$readelf"; do [[ -x "$input" ]] || missing "$input"; done
[[ "$(shasum -a 256 "$ndk/source.properties" | awk '{print $1}')" == \
  c00aa236fdb205e9be9edd9e2169763e48aca52735efff4e16f34205d49783b5 ]] ||
  fail 'NDK source.properties drift'
[[ "$(shasum -a 256 "$tc/../sysroot/usr/include/dlfcn.h" | awk '{print $1}')" == \
  134558a19eaed3763c7f2b304b91637f67820363569571da67c3ac257f338d90 ]] ||
  fail 'NDK dlfcn.h drift'
[[ "$(shasum -a 256 "$tc/../sysroot/usr/include/android/dlext.h" | awk '{print $1}')" == \
  c0d77220f274ea57f414f6b5add3a63ee705a5102a96cdaa2e41c4ed5fc62959 ]] ||
  fail 'NDK dlext.h drift'

tmp="$(mktemp -d "${TMPDIR:-/tmp}/bionic-guest-libdl.XXXXXX")"
trap 'find "$tmp" -depth -delete' EXIT

"$android_cc" -std=c17 -Wall -Wextra -Werror -c "$dir/probes/abi.c" \
  -o "$tmp/abi.o"

common=(-std=c17 -O2 -fno-builtin -fPIC -fno-stack-protector
        -U_FORTIFY_SOURCE -D_FORTIFY_SOURCE=0 -Wall -Wextra -Werror -shared
        -nostdlib -fuse-ld=lld -Wl,--build-id=none -Wl,--hash-style=sysv
        -Wl,-z,now -Wl,-z,norelro -Wl,-z,max-page-size=16384)
"$android_cc" "${common[@]}" -Wl,-soname,libguest_lifecycle.so \
  -Wl,--version-script,"$dir/probes/lifecycle.map" \
  "$dir/probes/lifecycle_stub.c" -o "$tmp/libguest_lifecycle.so"
"$android_cc" "${common[@]}" -Wl,-soname,libguest_missing.so \
  -Wl,--version-script,"$dir/probes/missing.map" \
  "$dir/probes/missing_stub.c" -o "$tmp/libguest_missing.so"
"$android_cc" "${common[@]}" -Wl,-soname,libguest_libdl_plugin.so \
  -Wl,--version-script,"$dir/probes/plugin.map" "$dir/probes/plugin.c" \
  "$tmp/libguest_lifecycle.so" -o "$tmp/libguest_libdl_plugin.so"
"$android_cc" "${common[@]}" -Wl,-soname,libguest_libdl_bad.so \
  "$dir/probes/bad_plugin.c" "$tmp/libguest_missing.so" \
  -o "$tmp/libguest_libdl_bad.so"
"$android_cc" "${common[@]}" -Wl,-soname,libguest_libdl_root.so \
  -Wl,--version-script,"$dir/probes/root.map" "$dir/probes/root.c" -ldl \
  -o "$tmp/libguest_libdl_root.so"

"$readelf" --dyn-syms --wide "$tmp/libguest_libdl_root.so" |
  awk '$7=="UND"&&$8!=""{name=$8;sub(/@.*/,"",name);print name}' | sort -u \
  > "$tmp/root.imports"
tail -n +2 "$dir/manifests/imports.tsv" | cut -f1 | sort -u > "$tmp/expected.imports"
diff -u "$tmp/expected.imports" "$tmp/root.imports" || fail 'root import drift'
"$readelf" --dyn-syms --wide "$tmp/libguest_libdl_root.so" |
  awk '$7=="UND"&&$8!=""&&$8!~ /@LIBC$/{bad=1}END{exit bad}' ||
  fail 'root import version drift'
"$readelf" -d "$tmp/libguest_libdl_root.so" | grep -F 'Shared library: [libdl.so]' >/dev/null ||
  fail 'root lacks libdl DT_NEEDED'
[[ "$("$readelf" --dyn-syms --wide "$tmp/libguest_libdl_root.so" |
  awk '$7!="UND"&&$5=="GLOBAL"&&$6=="DEFAULT"&&$8!=""{name=$8;sub(/@@.*/,"",name);print name}' |
  sort -u | tr '\n' ' ')" == \
  'JNI_OnLoad Java_dev_aim_probe_GuestLibdlFixture_nativePlugin ' ]] ||
  fail 'root export drift'
"$readelf" -d "$tmp/libguest_libdl_plugin.so" | grep -F 'Shared library: [libguest_lifecycle.so]' >/dev/null ||
  fail 'plugin lacks explicit lifecycle provider'
"$readelf" -d "$tmp/libguest_libdl_bad.so" | grep -F 'Shared library: [libguest_missing.so]' >/dev/null ||
  fail 'bad plugin lacks missing provider dependency'

ndk_libdl="$tc/../sysroot/usr/lib/aarch64-linux-android/$android_api/libdl.so"
[[ "$(shasum -a 256 "$ndk_libdl" | awk '{print $1}')" == \
  8ffe653ef85c82b83c84d3ae132cce9efee68900619c2f3a778a696e70664ce1 ]] ||
  fail 'NDK libdl stub drift'
"$readelf" --dyn-syms --wide "$ndk_libdl" |
  awk '$1~/^[0-9]+:$/&&$7!="UND"&&$8!=""{name=$8;sub(/@@/,"\t",name);print name}' |
  sort -u > "$tmp/full.exports"
tail -n +2 "$dir/manifests/full-libdl-exports.tsv" | cut -f1,2 | sort -u \
  > "$tmp/expected-full.exports"
diff -u "$tmp/expected-full.exports" "$tmp/full.exports" ||
  fail 'full NDK libdl export boundary drift'

[[ "$(tail -n +2 "$dir/upstream-sources.tsv" | wc -l | tr -d ' ')" == 14 ]] ||
  fail 'Android 16 source pin count drift'
[[ "$(tail -n +2 "$dir/manifests/android16-contract.tsv" | wc -l | tr -d ' ')" == 11 ]] ||
  fail 'Android 16 semantic contract drift'
[[ "$(awk -F '\t' 'NR>1&&$1=="platform/bionic"{print $3}' "$dir/upstream-sources.tsv" |
  sort -u)" == 09a271af557444c9a6b3f3146d6d474156fd6cdb ]] ||
  fail 'Android 16 Bionic revision drift'
[[ "$(awk -F '\t' 'NR>1&&$1=="platform/art"{print $3}' "$dir/upstream-sources.tsv" |
  sort -u)" == ed6c006bd06ae060bd9698fd2cb25c4865512ec3 ]] ||
  fail 'Android 16 ART revision drift'
while IFS=$'\t' read -r project _tag _revision path _bytes expected; do
  [[ "$project" == platform/art ]] || continue
  local_path="$root/_aosp/art-native-library-control-flow/$path"
  [[ -f "$local_path" ]] || missing "$local_path"
  [[ "$(shasum -a 256 "$local_path" | awk '{print $1}')" == "$expected" ]] ||
    fail "ART source drift: $path"
done < <(tail -n +2 "$dir/upstream-sources.tsv")

loader_target="$tmp/loader-target"
libdl_target="$tmp/libdl-target"
CARGO_TARGET_DIR="$loader_target" cargo build --quiet --release \
  --manifest-path "$root/crates/aim-elf-loader/Cargo.toml"
CARGO_TARGET_DIR="$libdl_target" cargo build --quiet --release \
  --manifest-path "$root/tools/android-dso-namespace/Cargo.toml" --lib
loader="$loader_target/release/libaim_elf_loader.a"
libdl="$libdl_target/release/libandroid_dso_namespace.a"
for input in "$loader" "$libdl"; do [[ -f "$input" ]] || missing "$input"; done

sdk="$(xcrun --sdk macosx --show-sdk-path)"
host_cxx="$(xcrun --find clang++)"
build_runner() {
  local sanitizer="$1"
  local output="$2"
  "$host_cxx" -arch arm64 -isysroot "$sdk" -std=c++20 -O1 -g \
    -Wall -Wextra -Werror -Wpedantic -fsanitize="$sanitizer" \
    -fno-omit-frame-pointer -I"$root/crates/aim-elf-loader/include" \
    -I"$root/tools/android-dso-namespace/include" \
    "$dir/probes/runner.cc" "$dir/probes/platform_stubs.cc" "$libdl" \
    "$loader" -framework Security -o "$output"
}
build_runner address,undefined "$tmp/runner-asan"
ASAN_OPTIONS=detect_leaks=0:halt_on_error=1 UBSAN_OPTIONS=halt_on_error=1 \
  "$tmp/runner-asan" "$tmp"
build_runner thread "$tmp/runner-tsan"
TSAN_OPTIONS=halt_on_error=1 "$tmp/runner-tsan" "$tmp"

# Rust's Darwin standard library has an unused backtrace-side `_dlsym` import.
# Guest relocation is nevertheless pinned to the five facade addresses, and
# no open/close/error entrypoint may come from Darwin.
if nm -u "$tmp/runner-asan" | rg '_(dlopen|dlclose|dlerror)$' >/dev/null; then
  fail 'runner imported a Darwin libdl lifecycle entrypoint'
fi
rg -q 'never consults `dlsym`' "$root/crates/aim-elf-loader/src/lib.rs" ||
  fail 'ELF loader closed-resolver contract drift'
if rg -n 'RTLD_GLOBAL|RTLD_DEFAULT|RTLD_NEXT|DYLD_|/usr/lib|\.dylib' \
  "$dir/probes/root.c" "$dir/probes/plugin.c" "$dir/probes/bad_plugin.c" \
  "$dir/probes/runner.cc" >/dev/null; then
  fail 'closed namespace escape in fixture'
fi

mkdir -p "$root/_build/bionic-guest-libdl-runtime"
cp "$tmp/libguest_libdl_root.so" "$tmp/libguest_libdl_plugin.so" \
  "$root/_build/bionic-guest-libdl-runtime/"
echo 'bionic-guest-libdl-runtime: PASS AndroidELF=JNI_OnLoad+named-JNI libdl.so@LIBC=5 sibling=exact refcount=same-handle ctor/fini=yes dlerror=TLS+consume close-lease=drained failure=rollback extinfo/flags=closed dyld=no ASan+UBSan+TSan activation=blocked-safe'
