#!/bin/bash
set -euo pipefail

project_root="$(cd "$(dirname "$0")/.." && pwd)"
lock="$project_root/upstream/android16-bionic-dso-namespace.lock"
# shellcheck disable=SC1090
source "$lock"

die() { echo "android-dso-namespace: $*" >&2; exit 2; }
sha() { shasum -a 256 "$1" | awk '{print $1}'; }
expect_sha() { [[ "$(sha "$1")" == "$2" ]] || die "SHA mismatch: $1"; }

sdk_root="${ANDROID_SDK_ROOT:-$HOME/Library/Android/sdk}"
ndk="$sdk_root/ndk/$NDK_REVISION"
prebuilt="$ndk/toolchains/llvm/prebuilt/darwin-x86_64"
sysroot="$prebuilt/sysroot"
android_lib="$sysroot/usr/lib/aarch64-linux-android"
readelf="$prebuilt/bin/llvm-readelf"
compiler="$prebuilt/bin/aarch64-linux-android${NDK_API}-clang++"
fixture_source="$project_root/tools/android-dso-namespace/tests/ndk_api35_fixture.cc"
libcxx="$android_lib/libc++_shared.so"
libdl="$android_lib/$NDK_API/libdl.so"
liblog_stub="$android_lib/$NDK_API/liblog.so"
liblog_archive="$project_root/_build/graphics-foundations/liblog-darwin.a"
liblog_map="$project_root/_aosp/system/logging/liblog/liblog.map.txt"

for path in "$readelf" "$compiler" "$libcxx" "$libdl" "$liblog_stub" \
  "$liblog_archive" "$liblog_map"; do
  [[ -e "$path" ]] || die "missing $path"
done

expect_sha "$ndk/source.properties" "$NDK_SOURCE_PROPERTIES_SHA256"
expect_sha "$sysroot/usr/include/dlfcn.h" "$NDK_DLFCN_H_SHA256"
expect_sha "$sysroot/usr/include/android/dlext.h" "$NDK_DLEXT_H_SHA256"
expect_sha "$fixture_source" "$NDK_FIXTURE_SOURCE_SHA256"
expect_sha "$libcxx" "$NDK_LIBCXX_SHARED_SHA256"
expect_sha "$libdl" "$NDK_LIBDL_STUB_SHA256"
expect_sha "$liblog_stub" "$NDK_LIBLOG_STUB_SHA256"
expect_sha "$liblog_map" "$AOSP_LIBLOG_MAP_SHA256"
expect_sha "$liblog_archive" "$AOSP_LIBLOG_DARWIN_ARCHIVE_SHA256"

work="$(mktemp -d "${TMPDIR:-/tmp}/android-dso-namespace.XXXXXX")"
trap 'rm -rf "$work"' EXIT

dyn_exports() {
  "$readelf" --dyn-syms --wide "$1" |
    awk '$1 ~ /^[0-9]+:$/ && $7 != "UND" && $8 != "" {sub(/@@.*/, "", $8); print $8}' |
    sort -u
}
dyn_imports() {
  "$readelf" --dyn-syms --wide "$1" |
    awk '$1 ~ /^[0-9]+:$/ && $7 == "UND" && $8 != "" {print $8}' |
    sort -u
}

dyn_imports "$libcxx" > "$work/libcxx.imports"
[[ "$(wc -l < "$work/libcxx.imports" | tr -d ' ')" == "$NDK_LIBCXX_IMPORT_COUNT" ]] || die "libc++ import count drift"
[[ "$(sha "$work/libcxx.imports")" == "$NDK_LIBCXX_IMPORT_MANIFEST_SHA256" ]] || die "libc++ import manifest drift"
[[ "$(awk -F@ '$2 == "LIBC" {count++} END {print count+0}' "$work/libcxx.imports")" == "$NDK_LIBCXX_LIBC_VERSIONED_IMPORT_COUNT" ]] || die "libc++ LIBC version count drift"
[[ "$(awk -F@ 'NF == 1 {count++} END {print count+0}' "$work/libcxx.imports")" == "$NDK_LIBCXX_UNVERSIONED_IMPORT_COUNT" ]] || die "libc++ unversioned count drift"
grep -Fx 'dl_iterate_phdr@LIBC' "$work/libcxx.imports" >/dev/null || die "libc++ no longer imports dl_iterate_phdr@LIBC"
cargo run --quiet --manifest-path "$project_root/tools/android-arm64-so-inspect/Cargo.toml" \
  -- "$libcxx" > "$work/libcxx.json"
[[ "$(grep -o '"dependency":"libc.so"' "$work/libcxx.json" | wc -l | tr -d ' ')" == "$NDK_LIBCXX_LIBC_SO_IMPORT_COUNT" ]] || die "libc++ libc.so attribution drift"
[[ "$(grep -o '"dependency":"libdl.so"' "$work/libcxx.json" | wc -l | tr -d ' ')" == "$NDK_LIBCXX_LIBDL_SO_IMPORT_COUNT" ]] || die "libc++ libdl.so attribution drift"

dyn_exports "$libdl" > "$work/libdl.exports"
dyn_exports "$liblog_stub" > "$work/liblog.exports"
[[ "$(wc -l < "$work/libdl.exports" | tr -d ' ')" == "$NDK_LIBDL_EXPORT_COUNT" ]] || die "libdl export count drift"
[[ "$(sha "$work/libdl.exports")" == "$NDK_LIBDL_EXPORT_MANIFEST_SHA256" ]] || die "libdl export manifest drift"
[[ "$(wc -l < "$work/liblog.exports" | tr -d ' ')" == "$NDK_LIBLOG_EXPORT_COUNT" ]] || die "liblog export count drift"
[[ "$(sha "$work/liblog.exports")" == "$NDK_LIBLOG_EXPORT_MANIFEST_SHA256" ]] || die "liblog export manifest drift"

cargo test --quiet --manifest-path "$project_root/tools/android-dso-namespace/Cargo.toml"
cargo build --quiet --manifest-path "$project_root/tools/android-dso-namespace/Cargo.toml" --lib
facade_archive="$project_root/target/debug/libandroid_dso_namespace.a"
[[ "$(lipo -archs "$facade_archive")" == "arm64" ]] || die "Rust facade archive is not Darwin arm64"
facade_definitions="$(nm -gU "$facade_archive" 2>/dev/null)"
for symbol in aim_loader_bind aim_dso_resolve aim_bionic_dlopen \
  aim_bionic_dlsym aim_bionic_dlclose aim_bionic_dlerror \
  aim_bionic_android_dlopen_ext; do
  grep -F " _$symbol" <<< "$facade_definitions" >/dev/null || die "facade archive lacks $symbol"
done
cargo run --quiet --manifest-path "$project_root/tools/android-dso-namespace/Cargo.toml" \
  --bin android-dso-namespace-smoke > "$work/provider.manifest"
awk -F '\t' '$1 == "libdl.so" {print $2}' "$work/provider.manifest" > "$work/provider.libdl"
awk -F '\t' '$1 == "liblog.so" {print $2}' "$work/provider.manifest" > "$work/provider.liblog"
[[ "$(wc -l < "$work/provider.libdl" | tr -d ' ')" == "$LOADER_LIBDL_FIRST_OWNER_COUNT" ]] || die "loader libdl owner count drift"
cmp -s "$work/provider.liblog" "$work/liblog.exports" || die "Rust liblog allowlist differs from API 35 NDK public stub"

archive_members="$(ar -t "$liblog_archive" | wc -l | tr -d ' ')"
[[ "$archive_members" == "$AOSP_LIBLOG_DARWIN_MEMBER_COUNT" ]] || die "liblog archive member count drift"
nm -gU "$liblog_archive" | awk '/ [TDSB] _/ {print substr($NF, 2)}' | sort -u > "$work/liblog.definitions"
[[ "$(wc -l < "$work/liblog.definitions" | tr -d ' ')" == "$AOSP_LIBLOG_DARWIN_DEFINITION_COUNT" ]] || die "liblog definition count drift"
[[ "$(sha "$work/liblog.definitions")" == "$AOSP_LIBLOG_DARWIN_DEFINITION_MANIFEST_SHA256" ]] || die "liblog definition manifest drift"
while IFS= read -r symbol; do
  grep -Fx "$symbol" "$work/liblog.definitions" >/dev/null || die "AOSP Darwin liblog lacks public symbol $symbol"
done < "$work/provider.liblog"

"$compiler" -shared -fPIC -Wl,--no-undefined "$fixture_source" -llog -ldl \
  -o "$work/libprovider_fixture.so"
cargo run --quiet --manifest-path "$project_root/tools/android-arm64-so-inspect/Cargo.toml" \
  -- "$work/libprovider_fixture.so" > "$work/fixture.json"
dyn_imports "$work/libprovider_fixture.so" > "$work/fixture.imports"
for symbol in android_dlopen_ext dlclose dlerror dlopen dlsym; do
  grep -Fx "$symbol@LIBC" "$work/fixture.imports" >/dev/null || die "fixture lacks $symbol@LIBC"
done
grep -Fx '__android_log_print' "$work/fixture.imports" >/dev/null || die "fixture lacks unversioned __android_log_print"
for needed in libc.so libm.so libdl.so liblog.so libc++_shared.so; do
  grep -F "Shared library: [$needed]" < <("$readelf" -d "$work/libprovider_fixture.so") >/dev/null || die "fixture lacks DT_NEEDED $needed"
done

echo "android-dso-namespace: PASS ndk=$NDK_REVISION api=$NDK_API libcxx-imports=$NDK_LIBCXX_IMPORT_COUNT"
echo "android-dso-namespace: providers=libdl.so:$LOADER_LIBDL_FIRST_OWNER_COUNT,liblog.so:$NDK_LIBLOG_EXPORT_COUNT closed-darwin-namespace=1"
echo "android-dso-namespace: intentional-next-blockers=libc.so,libm.so,libc++_shared.so,dl_iterate_phdr@LIBC"
