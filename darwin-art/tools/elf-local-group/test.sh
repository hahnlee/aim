#!/bin/bash
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
sdk="${ANDROID_SDK_ROOT:-$HOME/Library/Android/sdk}"
compiler="$sdk/ndk/28.2.13676358/toolchains/llvm/prebuilt/darwin-x86_64/bin/aarch64-linux-android35-clang"
stage="$(mktemp -d "${TMPDIR:-/tmp}/elf-local-group.XXXXXX")"
trap 'rm -rf "$stage"' EXIT
flags=(-shared -nostdlib -O2 -fPIC -fno-stack-protector -Wl,--hash-style=sysv
  -Wl,-z,now -Wl,-z,norelro -Wl,-z,max-page-size=16384)
"$compiler" "${flags[@]}" "$root/tools/elf-local-group/initialization.c" \
  -Wl,-soname,libinitialization.so -o "$stage/libinitialization.so"
cargo run --quiet --release --manifest-path "$root/crates/darwin-art-elf-loader/Cargo.toml" \
  --example linked_initialization -- "$stage/libinitialization.so"
"$compiler" "${flags[@]}" "$root/tools/elf-local-group/child.c" \
  -Wl,-soname,liblocal-child.so -o "$stage/liblocal-child.so"
"$compiler" "${flags[@]}" "$root/tools/elf-local-group/root.c" \
  "$stage/liblocal-child.so" -Wl,-soname,liblocal-root.so -o "$stage/liblocal-root.so"
cargo build --quiet --release --manifest-path "$root/crates/darwin-art-elf-loader/Cargo.toml" --lib
cargo build --quiet --release --manifest-path "$root/crates/darwin-art-runtime/Cargo.toml" --lib
xcrun clang++ -std=c++17 -arch arm64 -Wall -Wextra -Werror \
  -I"$root/crates/darwin-art-elf-loader/include" \
  "$root/tools/elf-local-group/selected_group_lifetime_test.cc" \
  -I"$root/include" -I"$root/compat/loader" \
  "$root/target/release/libdarwin_art_elf_loader.a" \
  "$root/target/release/libdarwin_art_runtime.a" \
  -framework Security -framework CoreFoundation -liconv -o "$stage/selected-group-test"
"$stage/selected-group-test" "$stage/liblocal-root.so" "$stage/liblocal-child.so"
"$compiler" "${flags[@]}" "$root/tools/elf-local-group/finalizer_order_provider.c" -Wl,-soname,liborder-provider.so -o "$stage/liborder-provider.so"
"$compiler" "${flags[@]}" -DORDER_NODE=3 "$root/tools/elf-local-group/finalizer_order.c" "$stage/liborder-provider.so" -Wl,-soname,liborder3.so -o "$stage/liborder3.so"
"$compiler" "${flags[@]}" -DORDER_NODE=1 "$root/tools/elf-local-group/finalizer_order.c" "$stage/liborder3.so" "$stage/liborder-provider.so" -Wl,-soname,liborder1.so -o "$stage/liborder1.so"
"$compiler" "${flags[@]}" -DORDER_NODE=2 "$root/tools/elf-local-group/finalizer_order.c" "$stage/liborder3.so" "$stage/liborder-provider.so" -Wl,-soname,liborder2.so -o "$stage/liborder2.so"
"$compiler" "${flags[@]}" -DORDER_NODE=0 "$root/tools/elf-local-group/finalizer_order.c" "$stage/liborder1.so" "$stage/liborder2.so" "$stage/liborder-provider.so" -Wl,-soname,liborder0.so -o "$stage/liborder0.so"
xcrun clang++ -std=c++17 -arch arm64 -Wall -Wextra -Werror \
  -I"$root/crates/darwin-art-elf-loader/include" "$root/tools/elf-local-group/finalizer_order_test.cc" \
  -I"$root/compat" "$root/compat/loader/namespace_group_release.cc" \
  "$root/compat/loader/namespace_close.cc" \
  -I"$root/include" "$root/target/release/libdarwin_art_runtime.a" \
  "$root/target/release/libdarwin_art_elf_loader.a" \
  -framework Security -framework CoreFoundation -liconv -o "$stage/finalizer-order-test"
"$stage/finalizer-order-test" "$stage/liborder0.so" "$stage/liborder1.so" "$stage/liborder2.so" "$stage/liborder3.so"
"$compiler" "${flags[@]}" "$root/tools/elf-local-group/initialization_provider.c" \
  -Wl,-soname,libinitialization-provider.so -o "$stage/libinitialization-provider.so"
"$compiler" "${flags[@]}" -DOBSERVE_LINKED_IMAGE "$root/tools/elf-local-group/initialization.c" \
  "$stage/libinitialization-provider.so" \
  -Wl,-soname,libinitialization.so -o "$stage/libinitialization-reentry.so"
xcrun clang++ -std=c++17 -arch arm64 -Wall -Wextra -Werror \
  -I"$root/crates/darwin-art-elf-loader/include" \
  -I"$root/include" \
  "$root/tools/elf-local-group/linked_initialization_test.cc" \
  "$root/target/release/libdarwin_art_elf_loader.a" "$root/target/release/libdarwin_art_runtime.a" \
  -framework Security -framework CoreFoundation -liconv -o "$stage/linked-init-test"
"$stage/linked-init-test" "$stage/libinitialization-reentry.so"
xcrun clang++ -std=c++17 -arch arm64 -Wall -Wextra -Werror \
  -I"$root/crates/darwin-art-elf-loader/include" "$root/tools/elf-local-group/test.cc" \
  "$root/tools/elf-local-group/selected_image_test.cc" \
  "$root/tools/elf-local-group/resident_admission_test.cc" \
  -I"$root/include" -I"$root/compat/loader" \
  "$root/compat/loader/namespace_elf_group.cc" \
  "$root/compat/loader/namespace_admission.cc" \
  "$root/compat/loader/namespace_resident_metadata.cc" \
  "$root/target/release/libdarwin_art_runtime.a" \
  "$root/target/release/libdarwin_art_elf_loader.a" \
  -framework Security -framework CoreFoundation -liconv -o "$stage/test"
"$stage/test" "$stage/liblocal-root.so" "$stage/liblocal-child.so" 42
cargo run --quiet --release --manifest-path "$root/crates/darwin-art-elf-loader/Cargo.toml" \
  --example namespace_scopes -- "$stage/liblocal-root.so" "$stage/liblocal-child.so"
cargo run --quiet --release --manifest-path "$root/crates/darwin-art-elf-loader/Cargo.toml" \
  --example resident_group -- "$stage/liblocal-root.so" "$stage/liblocal-child.so"
"$compiler" "${flags[@]}" -DIMPORT_PARENT "$root/tools/elf-local-group/child.c" \
  -Wl,-soname,liblocal-child.so -o "$stage/liblocal-child.so"
"$stage/test" "$stage/liblocal-root.so" "$stage/liblocal-child.so" 42
"$compiler" "${flags[@]}" -DPROTECTED_VALUE "$root/tools/elf-local-group/child.c" \
  -Wl,-soname,liblocal-child.so -o "$stage/liblocal-child.so"
"$stage/test" "$stage/liblocal-root.so" "$stage/liblocal-child.so" 16
# A new global child precedes the root's ordinary local definition.
"$compiler" "${flags[@]}" -Wl,-z,global "$root/tools/elf-local-group/child.c" \
  -Wl,-soname,liblocal-child.so -o "$stage/liblocal-child.so"
"${compiler%/*}/llvm-readelf" -d "$stage/liblocal-child.so" | grep 'FLAGS_1.*GLOBAL'
"$stage/test" "$stage/liblocal-root.so" "$stage/liblocal-child.so" 16
"$compiler" "${flags[@]}" -Wl,-z,global "$root/tools/elf-local-group/global.c" \
  -Wl,-soname,libglobal.so -o "$stage/libglobal.so"
cargo run --quiet --release --manifest-path "$root/crates/darwin-art-elf-loader/Cargo.toml" \
  --example namespace_scopes -- "$stage/liblocal-root.so" "$stage/liblocal-child.so" "$stage/libglobal.so"
"$compiler" "${flags[@]}" -Wl,-z,global "$root/tools/elf-local-group/child.c" \
  -Wl,-soname,liblocal-child.so -o "$stage/liblocal-child.so"
cargo run --quiet --release --manifest-path "$root/crates/darwin-art-elf-loader/Cargo.toml" \
  --example global_group -- "$stage/liblocal-root.so" "$stage/liblocal-child.so" "$stage/libglobal.so"
# Also cover an explicit DT_NEEDED on the already resident global image.
"$stage/test" "$stage/liblocal-root.so" "$stage/liblocal-child.so" 78 "$stage/libglobal.so"
"$compiler" "${flags[@]}" "$root/tools/elf-local-group/root.c" \
  "$stage/liblocal-child.so" -Wl,--no-as-needed "$stage/libglobal.so" \
  -Wl,-soname,liblocal-root.so -o "$stage/liblocal-root.so"
"$stage/test" "$stage/liblocal-root.so" "$stage/liblocal-child.so" 78 "$stage/libglobal.so"
cargo run --quiet --release --manifest-path "$root/crates/darwin-art-elf-loader/Cargo.toml" \
  --example global_group -- "$stage/liblocal-root.so" "$stage/liblocal-child.so" "$stage/libglobal.so"
# Keep execution rejection until Android logical group-close policy is wired.
"$compiler" "${flags[@]}" -Wl,-z,global -Wl,-z,nodelete "$root/tools/elf-local-group/global.c" \
  -Wl,-soname,libglobal.so -o "$stage/libglobal.so"
"${compiler%/*}/llvm-readelf" -d "$stage/libglobal.so" | rg 'FLAGS_1.*NODELETE'
"$stage/test" "$stage/liblocal-root.so" "$stage/liblocal-child.so" 78 "$stage/libglobal.so" expect-nodelete-rejection
