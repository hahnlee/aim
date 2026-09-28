#!/bin/bash
set -euo pipefail
export LC_ALL=C

project_root="$(cd "$(dirname "$0")/.." && pwd)"
source "$project_root/upstream/android16-liblog-exec-provider.lock"

fail() { echo "android-liblog-exec-provider: $*" >&2; exit 3; }
missing() { echo "android-liblog-exec-provider: missing $*" >&2; exit 2; }
sha() { shasum -a 256 "$1" | awk '{print $1}'; }
verify_sha() { [[ -f "$1" ]] || missing "$1"; [[ "$(sha "$1")" == "$2" ]] || fail "SHA drift: $1"; }

sdk_root="${ANDROID_SDK_ROOT:-$HOME/Library/Android/sdk}"
ndk="$sdk_root/ndk/$NDK_REVISION"
prebuilt="$ndk/toolchains/llvm/prebuilt/darwin-x86_64"
readelf="$prebuilt/bin/llvm-readelf"
ndk_liblog="$prebuilt/sysroot/usr/lib/aarch64-linux-android/$NDK_API/liblog.so"
aosp_root="$project_root/_aosp/system/logging/liblog"
aosp_archive="$project_root/_build/graphics-foundations/liblog-darwin.a"
provider_root="$project_root/tools/android-liblog-exec-provider"
build_dir="$project_root/_build/android-liblog-exec-provider"

[[ -x "$readelf" ]] || missing "$readelf"
verify_sha "$ndk_liblog" "$NDK_LIBLOG_STUB_SHA256"
verify_sha "$aosp_root/liblog.map.txt" "$AOSP_LIBLOG_MAP_SHA256"
verify_sha "$aosp_archive" "$AOSP_LIBLOG_DARWIN_ARCHIVE_SHA256"
[[ -f "$aosp_root/.source-revision" && "$(<"$aosp_root/.source-revision")" == "$SYSTEM_LOGGING_REVISION" ]] ||
  fail "AOSP system/logging revision drift"
[[ "$(ar -t "$aosp_archive" | wc -l | tr -d ' ')" == "$AOSP_LIBLOG_DARWIN_MEMBER_COUNT" ]] ||
  fail "AOSP liblog archive member drift"

stage="$(mktemp -d "${TMPDIR:-/tmp}/android-liblog-provider.XXXXXX")"
trap 'rm -rf "$stage"' EXIT

"$readelf" --dyn-syms --wide "$ndk_liblog" |
  awk '$1 ~ /^[0-9]+:$/ && $7 != "UND" && $8 != "" {sub(/@@.*/, "", $8); print $8}' |
  sort -u > "$stage/ndk-public.txt"
[[ "$(wc -l < "$stage/ndk-public.txt" | tr -d ' ')" == "$NDK_LIBLOG_EXPORT_COUNT" ]] ||
  fail "NDK public symbol count drift"
[[ "$(sha "$stage/ndk-public.txt")" == "$NDK_LIBLOG_EXPORT_MANIFEST_SHA256" ]] ||
  fail "NDK public symbol manifest drift"

cxx="$(xcrun --find clang++)"
libtool_bin="$(xcrun --find libtool)"
macos_sdk="$(xcrun --sdk macosx --show-sdk-path)"
flags=(
  -std=c++20 -arch arm64 -isysroot "$macos_sdk" -fPIC
  -Wall -Wextra -Werror
  -I"$provider_root/include"
  -I"$aosp_root/include"
)

"$cxx" "${flags[@]}" -c "$provider_root/liblog_provider.cc" -o "$stage/provider.o"
[[ "$(file "$stage/provider.o")" == *"Mach-O 64-bit object arm64"* ]] ||
  fail "provider object is not Darwin arm64"

provider_archive="$stage/libandroid-liblog-provider-table-darwin.a"
"$libtool_bin" -static -o "$provider_archive" "$stage/provider.o"
[[ "$(lipo -archs "$provider_archive")" == arm64 ]] || fail "provider archive is not arm64"
[[ "$(ar -t "$provider_archive" | grep -vc '^__\.SYMDEF')" == 1 ]] ||
  fail "provider archive must have one implementation member"

provider_dylib="$stage/libandroid-liblog-exec-provider.dylib"
"$cxx" -arch arm64 -isysroot "$macos_sdk" -dynamiclib \
  "$provider_archive" "$aosp_archive" \
  -Wl,-exported_symbols_list,"$provider_root/provider.exports" \
  -Wl,-install_name,@rpath/libandroid-liblog-exec-provider.dylib \
  -o "$provider_dylib"
[[ "$(lipo -archs "$provider_dylib")" == arm64 ]] || fail "provider dylib is not arm64"
if nm -u "$provider_dylib" | grep -E '^___android_log_' >/dev/null; then
  fail "provider dylib has unresolved AOSP liblog symbols"
fi
nm -gU "$provider_dylib" | awk '{print $3}' | sort -u > "$stage/dylib-exports.txt"
grep '^_' "$provider_root/provider.exports" | sort -u > "$stage/expected-dylib-exports.txt"
cmp -s "$stage/dylib-exports.txt" "$stage/expected-dylib-exports.txt" ||
  fail "dylib exports anything outside the four provider C ABI functions"
for private in ___android_log_write ___android_log_print _android_log_destroy; do
  if grep -Fx "$private" "$stage/dylib-exports.txt" >/dev/null; then
    fail "AOSP archive symbol leaked into Mach-O namespace: $private"
  fi
done

smoke="$stage/android-liblog-exec-provider-smoke"
"$cxx" "${flags[@]}" "$provider_root/provider_smoke.cc" "$provider_dylib" \
  -Wl,-rpath,"$build_dir" -o "$smoke"

mkdir -p "$build_dir"
cp "$provider_archive" "$build_dir/libandroid-liblog-provider-table-darwin.a"
cp "$provider_dylib" "$build_dir/libandroid-liblog-exec-provider.dylib"
cp "$smoke" "$build_dir/android-liblog-exec-provider-smoke"

"$build_dir/android-liblog-exec-provider-smoke" > "$stage/provider-manifest.tsv"
cut -f1 "$stage/provider-manifest.tsv" > "$stage/provider-names.txt"
{ cat "$stage/ndk-public.txt"; printf '%s\n' __android_log_error_write; } |
  sort -u > "$stage/expected-provider-names.txt"
cmp -s "$stage/provider-names.txt" "$stage/expected-provider-names.txt" ||
  fail "executable provider manifest differs from NDK API 35 plus reviewed extension"
[[ "$(wc -l < "$stage/provider-manifest.tsv" | tr -d ' ')" == "$((NDK_LIBLOG_EXPORT_COUNT + 1))" ]] ||
  fail "executable provider count drift"

echo "android-liblog-exec-provider: PASS arm64=1 ndk-public=18 extensions=1 nonzero=19 unique=19 calls=3 versions=unversioned+LIBLOG"
echo "android-liblog-exec-provider: archive=$build_dir/libandroid-liblog-provider-table-darwin.a"
echo "android-liblog-exec-provider: dylib=$build_dir/libandroid-liblog-exec-provider.dylib private-mach-o-exports=0"
