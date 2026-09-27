#!/bin/bash
set -euo pipefail
export LC_ALL=C

script_dir="$(cd "$(dirname "$0")" && pwd)"
fixture_dir="$script_dir/fixtures"
ndk_revision=28.2.13676358
ndk_root="${ANDROID_NDK_ROOT:-$HOME/Library/Android/sdk/ndk/$ndk_revision}"
toolchain="$ndk_root/toolchains/llvm/prebuilt/darwin-x86_64/bin"
clang="$toolchain/aarch64-linux-android35-clang"
linker="$toolchain/ld.lld"

[[ -x "$clang" && -x "$linker" ]] || {
  echo "Android NDK $ndk_revision is required via ANDROID_NDK_ROOT" >&2
  exit 2
}

stage="$(mktemp -d "${TMPDIR:-/tmp}/arm64-so-inspect-fixture.XXXXXX")"
trap 'rm -rf "$stage"' EXIT

"$clang" -c -fPIC "$fixture_dir/dependency.S" -o "$stage/dependency.o"
"$linker" -shared --soname=libfixture_dep.so --hash-style=both -z relro \
  --version-script="$fixture_dir/dependency.map" \
  "$stage/dependency.o" -o "$fixture_dir/libfixture_dep.so"

"$clang" -c -fPIC "$fixture_dir/smoke.S" -o "$stage/smoke.o"
"$linker" -shared --soname=libarm64_inspector_smoke.so --hash-style=both \
  -z relro -z now --init=smoke_constructor -rpath='$ORIGIN/fixture' \
  --version-script="$fixture_dir/smoke.map" \
  --no-as-needed "$fixture_dir/libfixture_dep.so" "$stage/smoke.o" \
  -o "$fixture_dir/libarm64_inspector_smoke.so"

for fixture in libarm64_inspector_smoke.so libfixture_dep.so; do
  shasum -a 256 "$fixture_dir/$fixture"
done
