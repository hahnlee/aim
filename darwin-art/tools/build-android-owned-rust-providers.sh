#!/bin/bash
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
providers="$root/_build/bionic-runtime-provider-closure"
out="$root/_build/android-owned-rust-providers"
mkdir -p "$out"
# Reuse compiled dependencies, but never replace the deployed provider archive.
# Callers serialize this with the legacy closure builder.
CARGO_TARGET_DIR="$providers/cargo-target" cargo build --quiet --release \
  --manifest-path "$root/tools/bionic-runtime-provider-closure/Cargo.toml" --no-default-features
archive="$providers/cargo-target/release/libbionic_runtime_provider_closure.a"
xcrun nm -gU "$archive" > "$out/defined-symbols.txt"
if rg -q ' _(AIBinder_|AParcel_|AStatus_|AServiceManager_|darwin_art_android_binder_ndk_resolve)' \
    "$out/defined-symbols.txt"; then
  echo "android-owned providers: legacy Binder NDK owner remains" >&2
  exit 1
fi
for symbol in darwin_art_bionic_fs_close_core darwin_art_fdsan_exchange \
  darwin_art_bionic_vm_mmap_core; do
  rg -q "_${symbol}$" "$out/defined-symbols.txt"
done
cp "$archive" "$out/libandroid-owned-rust-providers.a"
shasum -a 256 "$archive" "$root/tools/bionic-runtime-provider-closure/Cargo.toml" \
  "$root/tools/bionic-runtime-provider-closure/src/lib.rs" > "$out/source-identity.txt"
echo "Android-owned provider closure: Rust resource owners retained, legacy Binder NDK absent PASS"
