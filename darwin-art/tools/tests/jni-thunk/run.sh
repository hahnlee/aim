#!/usr/bin/env bash
# Regular JNI thunk x28 (managed SP) validation against the product
# unwindstack quick-frame registry. Needs `cargo xtask build` outputs.
set -euo pipefail
root="$(cd "$(dirname "$0")/../../.." && pwd)"
out="$(mktemp -d "${TMPDIR:-/tmp}/jni-thunk-managed-sp.XXXXXX")"
trap 'rm -rf -- "$out"' EXIT
unwind="$root/_build/runtime-unwindstack"
archives=(
  "$unwind/libunwindstack-mach-providers.a"
  "$unwind/libunwindstack-core-darwin.a"
  "$unwind/libunwindstack-dex-smoke-stub.a"
  "$unwind/rust-demangle-target/release/libdarwin_art_rust_demangle.a"
  "$root/_build/jit-compiler/libart-libelffile-darwin.a"
  "$root/_build/libbase-foundation/libandroid-base-darwin.a"
  "$root/_build/graphics-foundations/liblog-darwin.a"
)
for archive in "${archives[@]}"; do
  [[ -f "$archive" ]] || { echo "missing $archive; run cargo xtask build" >&2; exit 1; }
done
# <unwindstack/Elf.h> reads the Android ELF ABI header, as the product build does.
ndk_include=""
for ndk in ${ANDROID_NDK_HOME:-} ${ANDROID_NDK_ROOT:-} $(ls -rd "$HOME"/Library/Android/sdk/ndk/* 2>/dev/null); do
  for include in "$ndk"/toolchains/llvm/prebuilt/*/sysroot/usr/include; do
    if [[ -f "$include/elf.h" ]]; then ndk_include="$include"; break 2; fi
  done
done
[[ -n "$ndk_include" ]] || { echo "Android NDK headers (<elf.h>) are required" >&2; exit 1; }
xcrun clang++ -std=gnu++20 -arch arm64 -O2 \
  -DDARWIN_ART_PREINCLUDED_ELF -include "$ndk_include/elf.h" \
  -idirafter "$ndk_include" -idirafter "$ndk_include/aarch64-linux-android" \
  -I"$root/compat" \
  -I"$root/_aosp/system/unwinding/libunwindstack/include" \
  "$root/compat/darwin_android_jni_trampoline.cc" \
  "$root/tools/tests/jni-thunk/managed-sp-test.cc" \
  "${archives[@]}" -L/opt/homebrew/lib -lzstd -lz -o "$out/managed-sp-test"
"$out/managed-sp-test"
