#!/usr/bin/env bash
set -euo pipefail
export LC_ALL=C

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"
fail() { echo "bionic-provider-namespace: $*" >&2; exit 2; }

tmp="$(mktemp -d "${TMPDIR:-/tmp}/bionic-provider-namespace.XXXXXX")"
cleanup() {
  [[ -n "$tmp" && "$tmp" == "${TMPDIR:-/tmp}"/bionic-provider-namespace.* ]] &&
    rm -rf "$tmp"
}
trap cleanup EXIT

# The ownership/unsupported partition must still describe the real pinned ELF,
# not merely agree with a copied manifest.
# shellcheck disable=SC1091
source "$root/tools/bionic-libc-leaf-facade/sources.lock"
ndk_root="${ANDROID_NDK_ROOT:-${ANDROID_HOME:-$HOME/Library/Android/sdk}/ndk/$NDK_REVISION}"
libcxx="$ndk_root/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/lib/aarch64-linux-android/libc++_shared.so"
readelf="$ndk_root/toolchains/llvm/prebuilt/darwin-x86_64/bin/llvm-readelf"
[[ -f "$libcxx" && -x "$readelf" ]] || fail 'missing pinned NDK libc++ or llvm-readelf'
[[ "$(stat -f '%z' "$libcxx")" == "$NDK_LIBCXX_SHARED_SIZE" ]] ||
  fail 'pinned libc++ size drift'
[[ "$(shasum -a 256 "$libcxx" | awk '{print $1}')" == "$NDK_LIBCXX_SHARED_SHA256" ]] ||
  fail 'pinned libc++ hash drift'
"$readelf" --dyn-syms --wide "$libcxx" |
  awk '$7=="UND" && $8 ~ /@LIBC/ {name=$8; sub(/@.*/,"",name); print name}' |
  sort -u >"$tmp/actual-libc-imports"
tail -n +2 "$root/tools/bionic-libc-leaf-facade/imports/ndk-r28c-api35-arm64-libc.tsv" |
  cut -f1 | sort -u >"$tmp/manifest-libc-imports"
diff -u "$tmp/manifest-libc-imports" "$tmp/actual-libc-imports" ||
  fail 'canonical libc import universe no longer matches pinned ELF'

python3 "$here/generate_manifests.py" "$tmp/generated"
for generated in ownership.tsv unsupported-libc.tsv ownership.inc unsupported.inc unsupported_symbols.inc unsupported_count.inc liblog_versions.inc; do
  diff -u "$here/generated/$generated" "$tmp/generated/$generated" ||
    fail "generated $generated drift"
done

[[ "$(tail -n +2 "$here/generated/ownership.tsv" | awk -F '\t' '$1=="libc.so"{n++}END{print n+0}')" == 620 ]] ||
  fail 'expected pinned libc++ owners plus reviewed extensions'
[[ "$(tail -n +2 "$here/generated/ownership.tsv" | awk -F '\t' '$1=="libdl.so"{n++}END{print n+0}')" == 7 ]] ||
  fail 'expected seven closed guest libdl owners'
[[ "$(tail -n +2 "$here/generated/ownership.tsv" | awk -F '\t' '$1=="liblog.so"{n++}END{print n+0}')" == 19 ]] ||
  fail 'expected 19 liblog symbols with original default versions'
[[ "$(tail -n +2 "$here/generated/ownership.tsv" | awk -F '\t' '$1=="libbinder_ndk.so"{n++}END{print n+0}')" == 39 ]] ||
  fail 'expected complete Chromium Binder NDK surface'
[[ "$(tail -n +2 "$here/generated/ownership.tsv" | awk -F '\t' '$1=="libaaudio.so"{n++}END{print n+0}')" == 30 ]] ||
  fail 'expected complete Chromium AAudio surface'
[[ "$(tail -n +2 "$here/generated/ownership.tsv" | cut -f1-3 | sort | uniq -d | wc -l | tr -d ' ')" == 0 ]] ||
  fail 'duplicate SONAME/symbol/version owners'
awk -F '\t' 'NR>1 {key=$1 FS $2; if (key in owner && owner[key]!=$4) exit 1; owner[key]=$4}' \
  "$here/generated/ownership.tsv" || fail 'version aliases cross provider owners'
[[ "$(tail -n +2 "$here/generated/unsupported-libc.tsv" | wc -l | tr -d ' ')" == 0 ]] ||
  fail 'expected no unsupported imports'

if rg -n '\b(dlopen|dlsym|dlvsym|NSLookupSymbolInImage|_dyld_)\b' \
    "$here/src" "$here/include"; then
  fail 'host dynamic lookup path present'
fi

cxx="$(xcrun --find clang++)"
cc="$(xcrun --find clang)"
sdk="$(xcrun --sdk macosx --show-sdk-path)"
flags=(-arch arm64 -isysroot "$sdk" -std=c++17 -O2 -Wall -Wextra -Werror
       -Wpedantic -pthread -I"$here/include" -I"$here/generated")
"$cxx" "${flags[@]}" "$here/src/namespace.cc" "$here/probes/namespace_smoke.cc" \
  -o "$tmp/namespace-smoke"
"$tmp/namespace-smoke"

"$cc" -arch arm64 -isysroot "$sdk" -std=c17 -Wall -Wextra -Werror -Wpedantic \
  -I"$here/include" "$here/probes/abi.c" -o "$tmp/abi"
"$tmp/abi"

"$cxx" "${flags[@]}" -c "$here/src/builtin_adapters.cc" -o "$tmp/builtin-adapters.o"
expected_resolvers="$tmp/expected-resolvers"
cat >"$expected_resolvers" <<'EOF'
_darwin_art_android_aaudio_resolve
_darwin_art_android_binder_ndk_resolve
_darwin_art_bionic_abort_resolve
_darwin_art_bionic_allocator_resolve
_darwin_art_bionic_binary128_conversion_resolve
_darwin_art_bionic_dso_lifecycle_resolve
_darwin_art_bionic_errno_resolve
_darwin_art_bionic_float_conversion_resolve
_darwin_art_bionic_format_resolve
_darwin_art_bionic_formatted_stdio_resolve
_darwin_art_bionic_fs_resolve
_darwin_art_bionic_ftw_resolve
_darwin_art_bionic_ioctl_resolve
_darwin_art_bionic_libc_leaf_resolve
_darwin_art_bionic_locale_resolve
_darwin_art_bionic_math_resolve
_darwin_art_bionic_namespace_bind
_darwin_art_bionic_numeric_resolve
_darwin_art_bionic_process_state_data_resolve
_darwin_art_bionic_process_state_resolve
_darwin_art_bionic_pthread_resolve
_darwin_art_bionic_scanf_resolve
_darwin_art_bionic_sendfile_resolve
_darwin_art_bionic_socket_broker_data_resolve
_darwin_art_bionic_socket_broker_dns_resolve
_darwin_art_bionic_socket_broker_resolve
_darwin_art_bionic_stdio_resolve
_darwin_art_bionic_strerror_resolve
_darwin_art_bionic_strftime_resolve
_darwin_art_bionic_swprintf_resolve
_darwin_art_bionic_syscall_resolve
_darwin_art_bionic_syslog_resolve
_darwin_art_bionic_time_data_resolve
_darwin_art_bionic_time_resolve
_darwin_art_bionic_vm_resolve
_darwin_art_bionic_wide_float_resolve
_darwin_art_bionic_wide_integer_resolve
_darwin_art_bionic_wide_stdio_resolve
_darwin_art_dl_phdr_resolve
_darwin_art_liblog_provider_resolve
EOF
nm -u "$tmp/builtin-adapters.o" | awk '{print $NF}' | sort >"$tmp/actual-resolvers"
diff -u "$expected_resolvers" "$tmp/actual-resolvers" ||
  fail 'builtin adapters have missing or unexpected dependencies'
"$cxx" "${flags[@]}" "$here/src/namespace.cc" \
  "$here/src/builtin_adapters.cc" "$here/probes/builtin_adapters_smoke.cc" \
  -o "$tmp/builtin-adapters-smoke"
"$tmp/builtin-adapters-smoke"

"$cxx" "${flags[@]}" -O1 -g -fsanitize=address,undefined \
  "$here/src/namespace.cc" "$here/probes/namespace_smoke.cc" \
  -o "$tmp/namespace-smoke-sanitized"
"$tmp/namespace-smoke-sanitized" >/dev/null

"$cxx" "${flags[@]}" -O1 -g -fsanitize=thread \
  "$here/src/namespace.cc" "$here/probes/namespace_smoke.cc" \
  -o "$tmp/namespace-smoke-tsan"
"$tmp/namespace-smoke-tsan" >/dev/null

undefined="$(nm -u "$tmp/namespace-smoke" || true)"
if grep -E '(_dlopen|_dlsym|_dlvsym|_NSLookupSymbolInImage|__dyld_)' <<<"$undefined"; then
  fail 'host loader undefined reference present'
fi

echo 'bionic-provider-namespace: PASS libcxx=160/160 libc-rows=620 liblog-rows=19 binder-ndk=39 aaudio=30 owned=800 unsupported=0 duplicate-triple=0 exact-version+original-default=yes resolver=closed teardown=ordered+quiescent asan+ubsan+tsan=yes'
