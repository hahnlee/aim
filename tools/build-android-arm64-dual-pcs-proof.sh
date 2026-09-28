#!/bin/bash
set -euo pipefail
export LC_ALL=C

project_root="$(cd "$(dirname "$0")/.." && pwd)"
source "$project_root/upstream/android-arm64-dual-pcs-proof.lock"

fail() { echo "android-arm64-pcs-proof: $*" >&2; exit 3; }
missing() { echo "android-arm64-pcs-proof: missing $*" >&2; exit 2; }
sha() { shasum -a 256 "$1" | awk '{print $1}'; }
verify_sha() { [[ -f "$1" ]] || missing "$1"; [[ "$(sha "$1")" == "$2" ]] || fail "SHA drift: $1"; }

sdk_root="${ANDROID_SDK_ROOT:-$HOME/Library/Android/sdk}"
ndk="$sdk_root/ndk/$NDK_REVISION"
prebuilt="$ndk/toolchains/llvm/prebuilt/darwin-x86_64"
android_clang="$prebuilt/bin/aarch64-linux-android${NDK_API}-clang"
readelf="$prebuilt/bin/llvm-readelf"
nm_android="$prebuilt/bin/llvm-nm"
objdump_android="$prebuilt/bin/llvm-objdump"
source_root="$project_root/tools/android-arm64-pcs-proof"
build_dir="$project_root/_build/android-arm64-pcs-proof"

for tool in "$android_clang" "$readelf" "$nm_android" "$objdump_android"; do
  [[ -x "$tool" ]] || missing "$tool"
done
verify_sha "$ndk/source.properties" "$NDK_SOURCE_PROPERTIES_SHA256"
verify_sha "$source_root/pcs_thunks.S" "$PCS_ASSEMBLY_SHA256"
verify_sha "$source_root/pcs_smoke.cc" "$PCS_CPP_SHA256"

stage="$(mktemp -d "${TMPDIR:-/tmp}/android-arm64-pcs-proof.XXXXXX")"
trap 'rm -rf "$stage"' EXIT

android_fixture="$stage/libandroid-arm64-pcs-fixture.so"
"$android_clang" -shared -fPIC -O2 -nostdlib -fuse-ld=lld \
  -Wl,--hash-style=sysv -Wl,--build-id=none \
  -Wl,-z,norelro \
  -Wl,-z,max-page-size=16384 -Wl,-z,common-page-size=16384 \
  -Wl,-soname,libandroid-arm64-pcs-fixture.so \
  "$source_root/pcs_thunks.S" -o "$android_fixture"
[[ "$(sha "$android_fixture")" == "$ANDROID_ELF_FIXTURE_SHA256" ]] ||
  fail "Android ELF fixture drift"
file "$android_fixture" | grep -F 'ELF 64-bit LSB shared object, ARM aarch64' >/dev/null ||
  fail "fixture is not Android AArch64 ELF"
if "$readelf" -d -r "$android_fixture" | grep -E '\(NEEDED\)|R_AARCH64_' >/dev/null; then
  fail "fixture must remain dependency- and relocation-free"
fi
for symbol in register_direct_target android_spilled_target android_to_darwin_repack \
  android_callback_fixture pcs_loader_smoke; do
  "$nm_android" --defined-only "$android_fixture" | grep -E " [Tt] $symbol$" >/dev/null ||
    fail "Android fixture lacks $symbol"
done
"$nm_android" -D --defined-only "$android_fixture" | grep -E ' T pcs_loader_smoke$' >/dev/null ||
  fail "loader smoke must be the sole callable public export"
[[ "$("$nm_android" -D --defined-only "$android_fixture" | awk '$2 == "T" {count++} END {print count+0}')" == 1 ]] ||
  fail "unexpected public fixture exports"

"$objdump_android" -d "$android_fixture" > "$stage/android-disassembly.txt"
grep -F 'ldrsb' "$stage/android-disassembly.txt" >/dev/null || fail "signed B load absent"
grep -F 'ldrsh' "$stage/android-disassembly.txt" >/dev/null || fail "signed S load absent"
grep -F 'ldrsw' "$stage/android-disassembly.txt" >/dev/null || fail "signed I load absent"

cxx="$(xcrun --find clang++)"
macos_sdk="$(xcrun --sdk macosx --show-sdk-path)"
native_smoke="$stage/android-arm64-pcs-native-smoke"
"$cxx" -std=c++20 -O2 -arch arm64 -isysroot "$macos_sdk" \
  -Wall -Wextra -Werror "$source_root/pcs_smoke.cc" "$source_root/pcs_thunks.S" \
  -o "$native_smoke"
[[ "$(file "$native_smoke")" == *'Mach-O 64-bit executable arm64'* ]] ||
  fail "native smoke is not Darwin arm64"
native_output="$("$native_smoke")"
grep -F "register-direct=PASS digest=$REGISTER_DIRECT_DIGEST" <<< "$native_output" >/dev/null ||
  fail "register-only direct digest mismatch"
grep -F "darwin-to-android-spill=PASS digest=$SPILLED_BIDIRECTIONAL_DIGEST" <<< "$native_output" >/dev/null ||
  fail "Darwin to Android spill digest mismatch"
grep -F "android-to-darwin-spill=PASS digest=$SPILLED_BIDIRECTIONAL_DIGEST" <<< "$native_output" >/dev/null ||
  fail "Android to Darwin spill digest mismatch"

mkdir -p "$build_dir"
cp "$android_fixture" "$build_dir/libandroid-arm64-pcs-fixture.so"
cp "$native_smoke" "$build_dir/android-arm64-pcs-native-smoke"

# Consumer-only use: this crate neither patches nor reaches into loader internals.
cargo run --quiet --manifest-path "$source_root/Cargo.toml" -- \
  "$build_dir/libandroid-arm64-pcs-fixture.so" \
  "$build_dir/android-arm64-pcs-native-smoke"

echo "android-arm64-pcs-proof: PASS direct=register-only darwin-stack=$DARWIN_SPILL_BYTES android-stack=$ANDROID_SPILL_BYTES"
echo "android-arm64-pcs-proof: scope=fixed-prototype,nonvariadic; JNI-table-generator=not-yet-claimed"
