#!/bin/bash
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
out="$root/_build/native-loader-policy"
cargo build --quiet --release --manifest-path "$root/Cargo.toml" -p darwin-art-runtime
xcrun clang++ -std=c++20 -arch arm64 -I"$root/compat" \
  -I"$root/tools/bionic-provider-namespace/include" \
  -I"$root/tools/bionic-provider-namespace/generated" -I"$root/include" \
  -I"$root/crates/darwin-art-elf-loader/include" \
  "$root/tools/bionic-provider-namespace/src/namespace.cc" \
  "$root/compat/loader/bionic_provider_set.cc" \
  "$root/compat/loader/bionic_symbol_lookup.cc" \
  "$root/compat/loader/android_unwind_image.cc" \
  "$root/compat/loader/bionic_provider_image.cc" \
  "$root/compat/loader/namespace_admission.cc" \
  "$root/tools/native-loader-policy/bionic_provider_set_test.cc" \
  "$root/_build/runtime-graphics-link-probe/libdarwin_art_runtime_graphics.dylib" \
  "$root/target/release/libdarwin_art_runtime.a" \
  "$root/target/release/libdarwin_art_elf_loader.a" \
  -framework Security -framework CoreFoundation -liconv \
  -Wl,-rpath,"$root/_build/runtime-graphics-link-probe" \
  -Wl,-dead_strip -o "$out/provider-set-test"
"$out/provider-set-test"
