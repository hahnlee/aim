#!/bin/bash
set -euo pipefail
export LC_ALL=C

project_root="$(cd "$(dirname "$0")/.." && pwd)"
source "$project_root/upstream/android35-libm-facade.lock"

fail() { echo "android-libm-facade: $*" >&2; exit 3; }
missing() { echo "android-libm-facade: missing $*" >&2; exit 2; }
sha() { shasum -a 256 "$1" | awk '{print $1}'; }
verify_sha() { [[ -f "$1" ]] || missing "$1"; [[ "$(sha "$1")" == "$2" ]] || fail "SHA drift: $1"; }

sdk_root="${ANDROID_SDK_ROOT:-$HOME/Library/Android/sdk}"
ndk="$sdk_root/ndk/$NDK_REVISION"
prebuilt="$ndk/toolchains/llvm/prebuilt/darwin-x86_64"
android_lib="$prebuilt/sysroot/usr/lib/aarch64-linux-android"
readelf="$prebuilt/bin/llvm-readelf"
android_clang="$prebuilt/bin/aarch64-linux-android${NDK_API}-clang"
libcxx="$android_lib/libc++_shared.so"
libm_stub="$android_lib/$NDK_API/libm.so"
source_root="$project_root/tools/android-libm-facade"
build_dir="$project_root/_build/android-libm-facade"

for path in "$readelf" "$android_clang" "$libcxx" "$libm_stub"; do
  [[ -e "$path" ]] || missing "$path"
done
verify_sha "$ndk/source.properties" "$NDK_SOURCE_PROPERTIES_SHA256"
verify_sha "$libcxx" "$NDK_LIBCXX_SHARED_SHA256"
verify_sha "$libm_stub" "$NDK_LIBM_STUB_SHA256"

stage="$(mktemp -d "${TMPDIR:-/tmp}/android-libm-facade.XXXXXX")"
trap 'rm -rf "$stage"' EXIT

"$readelf" -d "$libcxx" | awk '/\(NEEDED\)/ {gsub(/.*\[|\].*/, "", $0); print}' \
  > "$stage/libcxx-needed.txt"
[[ "$(wc -l < "$stage/libcxx-needed.txt" | tr -d ' ')" == "$NDK_LIBCXX_NEEDED_COUNT" &&
   "$(sha "$stage/libcxx-needed.txt")" == "$NDK_LIBCXX_NEEDED_MANIFEST_SHA256" ]] ||
  fail "libc++ DT_NEEDED manifest drift"
grep -Fx 'libm.so' "$stage/libcxx-needed.txt" >/dev/null || fail "libc++ no longer DT_NEEDED libm.so"

cargo run --quiet --manifest-path "$project_root/tools/android-arm64-so-inspect/Cargo.toml" -- \
  "$libcxx" > "$stage/libcxx.json"
jq -r '.versioning.requirements[] | .file as $file | .versions[] |
       [$file, .name] | @tsv' "$stage/libcxx.json" |
  sort > "$stage/libcxx-verneed.txt"
[[ "$(wc -l < "$stage/libcxx-verneed.txt" | tr -d ' ')" == "$NDK_LIBCXX_VERNEED_COUNT" &&
   "$(sha "$stage/libcxx-verneed.txt")" == "$NDK_LIBCXX_VERNEED_MANIFEST_SHA256" ]] ||
  fail "libc++ VERNEED manifest drift"
libcxx_libm_verneed_count="$(awk -F '\t' '$1 == "libm.so" {count++} END {print count+0}' \
  "$stage/libcxx-verneed.txt")"
[[ "$libcxx_libm_verneed_count" == "$NDK_LIBCXX_LIBM_VERNEED_COUNT" ]] ||
  fail "libc++ libm VERNEED count drift"

"$readelf" --dyn-syms --wide "$libcxx" |
  awk '$1 ~ /^[0-9]+:$/ && $7 == "UND" && $8 != "" {name=$8; sub(/@.*/, "", name); print name}' |
  sort -u > "$stage/libcxx-import-names.txt"
"$readelf" --dyn-syms --wide "$libm_stub" |
  awk '$1 ~ /^[0-9]+:$/ && $7 != "UND" && $8 != "" {
         exact=$8; sub(/@@/, "@", exact); print exact > versioned;
         name=$8; sub(/@.*/, "", name); print name > names
       }' versioned="$stage/libm-versioned.txt" names="$stage/libm-names.unsorted"
sort -u "$stage/libm-versioned.txt" -o "$stage/libm-versioned.txt"
sort -u "$stage/libm-names.unsorted" > "$stage/libm-names.txt"
[[ "$(wc -l < "$stage/libm-versioned.txt" | tr -d ' ')" == "$NDK_LIBM_EXPORT_COUNT" &&
   "$(sha "$stage/libm-versioned.txt")" == "$NDK_LIBM_VERSIONED_EXPORT_MANIFEST_SHA256" ]] ||
  fail "NDK libm dynsym/version manifest drift"
comm -12 "$stage/libcxx-import-names.txt" "$stage/libm-names.txt" > "$stage/libcxx-libm-intersection.txt"
libcxx_libm_import_count="$(wc -l < "$stage/libcxx-libm-intersection.txt" | tr -d ' ')"
[[ "$libcxx_libm_import_count" == "$NDK_LIBCXX_LIBM_DYNSYM_IMPORT_COUNT" ]] ||
  fail "libc++ libm import intersection drift"

android_fixture="$stage/libandroid-libm-used-subset.so"
"$android_clang" -shared -fPIC -O0 -fno-builtin -Wl,--no-undefined \
  "$source_root/ndk_used_subset.c" -lm -o "$android_fixture"
cargo run --quiet --manifest-path "$project_root/tools/android-arm64-so-inspect/Cargo.toml" -- \
  "$android_fixture" > "$stage/fixture.json"
jq -r '.symbols.imports[] |
       select(.version.dependency == "libm.so") |
       "\(.name)@\(.version.name)"' "$stage/fixture.json" |
  sort > "$stage/fixture-libm-imports.txt"
[[ "$(wc -l < "$stage/fixture-libm-imports.txt" | tr -d ' ')" == "$SAFE_SUBSET_COUNT" &&
   "$(sha "$stage/fixture-libm-imports.txt")" == "$SAFE_SUBSET_MANIFEST_SHA256" ]] ||
  fail "NDK fixture safe used subset drift"

cxx="$(xcrun --find clang++)"
libtool_bin="$(xcrun --find libtool)"
macos_sdk="$(xcrun --sdk macosx --show-sdk-path)"
flags=(-std=c++20 -arch arm64 -isysroot "$macos_sdk" -fPIC -frounding-math
       -Wall -Wextra -Werror -I"$source_root/include")
"$cxx" "${flags[@]}" -c "$source_root/libm_facade.cc" -o "$stage/libm_facade.o"
facade_archive="$stage/libandroid-libm-facade-darwin.a"
"$libtool_bin" -static -o "$facade_archive" "$stage/libm_facade.o"
[[ "$(lipo -archs "$facade_archive")" == arm64 ]] || fail "facade archive is not Darwin arm64"
definitions="$(nm -gU "$facade_archive")"
for symbol in aim_bionic_fabs aim_bionic_fabsf \
  aim_bionic_copysign aim_bionic_copysignf \
  aim_libm_resolve aim_libm_capability; do
  grep -F " _$symbol" <<< "$definitions" >/dev/null || fail "facade lacks $symbol"
done
if grep -E ' T _(fabs|fabsf|copysign|copysignf)$' <<< "$definitions" >/dev/null; then
  fail "unprefixed math interposition leaked from facade"
fi

smoke="$stage/android-libm-facade-smoke"
"$cxx" "${flags[@]}" "$source_root/libm_facade_smoke.cc" "$facade_archive" -o "$smoke"
smoke_output="$("$smoke")"
grep -F 'PASS safe=4 nan-payload=preserved signed-zero=preserved' <<< "$smoke_output" >/dev/null ||
  fail "edge differential smoke failed"
grep -F 'errno=unchanged fenv=unchanged rounding-mode=independent' <<< "$smoke_output" >/dev/null ||
  fail "environment differential smoke failed"

mkdir -p "$build_dir"
cp "$facade_archive" "$build_dir/libandroid-libm-facade-darwin.a"
cp "$smoke" "$build_dir/android-libm-facade-smoke"
cp "$android_fixture" "$build_dir/libandroid-libm-used-subset.so"

printf '%s\n' "$smoke_output"
echo "android-libm-facade: PASS libcxx-needed-libm=1 libcxx-libm-imports=0 libcxx-libm-verneed=0"
echo "android-libm-facade: provider-safe=4 version=LIBC Darwin-global-fallback=0"
