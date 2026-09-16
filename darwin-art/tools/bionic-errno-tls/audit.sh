#!/bin/bash
set -euo pipefail
export LC_ALL=C

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/../.." && pwd)"
lock="$script_dir/sources.lock"
fail() { echo "bionic-errno-tls: $1" >&2; exit 2; }
[[ -f "$lock" ]] || fail 'missing sources.lock'
# shellcheck disable=SC1090
source "$lock"

sha256() { shasum -a 256 "$1" | awk '{print $1}'; }
check_hash() {
  [[ "$(sha256 "$1")" == "$2" ]] || fail "hash mismatch: $1"
}

check_hash "$project_root/upstream/android16-os-constants.lock" \
  "$OS_CONSTANTS_LOCK_SHA256"
check_hash "$project_root/upstream/android16-os-constants-values.tsv" \
  "$OS_CONSTANTS_VALUES_SHA256"
check_hash "$project_root/compat/darwin_os_constants.cc" \
  "$OS_CONSTANTS_CPP_SHA256"
check_hash "$project_root/tools/build-android16-os-constants-darwin.sh" \
  "$OS_CONSTANTS_BUILD_GATE_SHA256"
grep -F "BIONIC_REVISION=$BIONIC_REVISION" \
  "$project_root/upstream/android16-os-constants.lock" >/dev/null ||
  fail 'Bionic revision differs from OsConstants gate'
grep -F 'include "android16_os_constants_errno.inc"' \
  "$project_root/compat/darwin_os_constants.cc" >/dev/null ||
  fail 'OsConstants errno mapping ownership drift'

ndk_root="${ANDROID_NDK_ROOT:-$HOME/Library/Android/sdk/ndk/$NDK_REVISION}"
toolchain="$ndk_root/toolchains/llvm/prebuilt/darwin-x86_64/bin"
android_cc="$toolchain/aarch64-linux-android35-clang"
readelf="$toolchain/llvm-readelf"
[[ -x "$android_cc" && -x "$readelf" ]] || fail 'missing pinned NDK toolchain'
check_hash "$ndk_root/source.properties" "$NDK_SOURCE_PROPERTIES_SHA256"
check_hash "$ndk_root/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include/errno.h" \
  "$NDK_ERRNO_HEADER_SHA256"

temp_root="$(mktemp -d "${TMPDIR:-/tmp}/bionic-errno-tls.XXXXXX")"
cleanup() {
  if [[ -n "$temp_root" && "$temp_root" == "${TMPDIR:-/tmp}"/bionic-errno-tls.* ]]; then
    find "$temp_root" -depth -delete
  fi
}
trap cleanup EXIT

fetch_bionic() {
  local source_file="$1"
  local expected_hash="$2"
  local output_file="$temp_root/$(basename "$source_file")"
  curl -fsSL "https://android.googlesource.com/platform/bionic/+/$BIONIC_REVISION/$source_file?format=TEXT" |
    base64 --decode >"$output_file"
  check_hash "$output_file" "$expected_hash"
}
fetch_bionic libc/bionic/__errno.cpp "$BIONIC_ERRNO_CPP_SHA256"
fetch_bionic libc/include/errno.h "$BIONIC_ERRNO_HEADER_SHA256"
fetch_bionic libc/private/bionic_tls.h "$BIONIC_TLS_HEADER_SHA256"
grep -F 'return &__get_thread()->errno_value;' "$temp_root/__errno.cpp" >/dev/null ||
  fail 'Bionic per-thread errno ownership drift'

values="$project_root/upstream/android16-os-constants-values.tsv"
python3 "$script_dir/tools/generate_mapping.py" "$values" probe \
  >"$temp_root/mapping_probe.c"
host_cc="$(xcrun --find clang)"
sdk_root="$(xcrun --sdk macosx --show-sdk-path)"
host_flags=(-arch arm64 -isysroot "$sdk_root" -std=c17 -O2 -Wall -Wextra
            -Werror -Wpedantic -I"$script_dir/include" -I"$script_dir/generated")
"$host_cc" "${host_flags[@]}" "$temp_root/mapping_probe.c" \
  -o "$temp_root/mapping-probe"
"$temp_root/mapping-probe" >"$temp_root/darwin-to-android.tsv"
diff -u "$script_dir/manifests/darwin-to-android.tsv" \
  "$temp_root/darwin-to-android.tsv" || fail 'Darwin errno manifest drift'
check_hash "$script_dir/manifests/darwin-to-android.tsv" \
  "$DARWIN_ANDROID_MANIFEST_SHA256"
python3 "$script_dir/tools/generate_mapping.py" \
  "$script_dir/manifests/darwin-to-android.tsv" table >"$temp_root/mapping.inc"
diff -u "$script_dir/generated/darwin_to_android.inc" "$temp_root/mapping.inc" ||
  fail 'generated mapping drift'
check_hash "$script_dir/generated/darwin_to_android.inc" "$GENERATED_MAPPING_SHA256"
[[ "$(wc -l < "$script_dir/manifests/darwin-to-android.tsv" | tr -d ' ')" == 79 ]] ||
  fail 'expected 79 Darwin-defined entries from 80 Android errno names'
awk -F '\t' '{if (($2 in seen) && seen[$2] != $3) exit 1; seen[$2]=$3}' \
  "$script_dir/manifests/darwin-to-android.tsv" ||
  fail 'one Darwin errno maps to conflicting Android values'

"$host_cc" "${host_flags[@]}" -c "$script_dir/src/errno_tls.c" \
  -o "$temp_root/errno_tls.o"
nm -u "$temp_root/errno_tls.o" | sed 's/^[[:space:]]*//' | sort \
  >"$temp_root/undefined"
cat >"$temp_root/expected-undefined" <<'EOF'
___error
__tlv_bootstrap
EOF
diff -u "$temp_root/expected-undefined" "$temp_root/undefined" ||
  fail 'TLS/host errno dependency drift'
definitions="$(nm -gU "$temp_root/errno_tls.o")"
for symbol in __errno errno_capture_host errno_from_darwin errno_to_darwin errno_load \
              errno_publish_result errno_resolve errno_set_from_darwin errno_store; do
  grep -F " _darwin_art_bionic_$symbol" <<<"$definitions" >/dev/null ||
    fail "missing prefixed definition $symbol"
done
if awk '$2 ~ /^[TDS]$/ {print $3}' <<<"$definitions" |
   grep -Ev '^_darwin_art_bionic_' >/dev/null; then
  fail 'unprefixed global definition escaped errno module'
fi
if rg -n 'dlsym|RTLD_|__error\(' "$script_dir/src" "$script_dir/include" >/dev/null; then
  fail 'host errno address lookup/exposure entered module'
fi

"$host_cc" "${host_flags[@]}" "$script_dir/src/errno_tls.c" \
  "$script_dir/probes/native_smoke.c" -o "$temp_root/native-smoke"
"$temp_root/native-smoke"
"$host_cc" "${host_flags[@]}" -O1 -g -fsanitize=address,undefined \
  "$script_dir/src/errno_tls.c" "$script_dir/probes/native_smoke.c" \
  -o "$temp_root/native-smoke-sanitized"
"$temp_root/native-smoke-sanitized" >/dev/null

allocator="$project_root/tools/bionic-libc-allocator-facade"
check_hash "$allocator/src/allocator.c" "$ALLOCATOR_SOURCE_SHA256"
check_hash "$allocator/src/allocator_options.c" "$ALLOCATOR_OPTIONS_SOURCE_SHA256"
"$host_cc" "${host_flags[@]}" -I"$allocator/include" \
  "$script_dir/src/errno_tls.c" "$allocator/src/allocator.c" \
  "$allocator/src/allocator_options.c" \
  "$script_dir/probes/allocator_seam.c" -o "$temp_root/allocator-seam"
"$temp_root/allocator-seam"

fixture="$temp_root/libbionic_errno_fixture.so"
"$android_cc" -std=c17 -O2 -fPIC -fno-stack-protector -Wall -Wextra \
  -Werror -Wpedantic -shared -nostdlib -Wl,-soname,libbionic_errno_fixture.so \
  -Wl,-z,now -Wl,-z,norelro -Wl,--hash-style=sysv \
  "$script_dir/probes/fixture.c" -o "$fixture"
"$readelf" -h "$fixture" | grep -F 'Machine:                           AArch64' >/dev/null ||
  fail 'fixture is not Android AArch64 ELF'
"$readelf" --dyn-syms --wide "$fixture" >"$temp_root/dynsyms"
awk '$7=="UND" && $8!="" {print $8}' "$temp_root/dynsyms" | sort -u \
  >"$temp_root/fixture-undefined"
cat >"$temp_root/expected-fixture-undefined" <<'EOF'
__errno
darwin_art_errno_fixture_thread_value
EOF
diff -u "$temp_root/expected-fixture-undefined" "$temp_root/fixture-undefined" ||
  fail 'Android ELF import namespace drift'
grep -E 'GLOBAL DEFAULT +[0-9]+ bionic_errno_fixture_run$' \
  "$temp_root/dynsyms" >/dev/null || fail 'fixture runner export missing'

CARGO_TARGET_DIR="$temp_root/cargo-target" cargo run --quiet \
  --manifest-path "$script_dir/Cargo.toml" -- "$fixture"
CARGO_TARGET_DIR="$temp_root/cargo-target" cargo clippy --quiet \
  --manifest-path "$script_dir/Cargo.toml" -- -D warnings
cargo fmt --manifest-path "$script_dir/Cargo.toml" -- --check

echo 'bionic-errno-tls: PASS AndroidELF=__errno pthreads=2 host-isolated mappings=79 closed-resolver ASAN+UBSAN'
