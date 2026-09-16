#!/bin/bash
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
src="$root/_aosp/bionic-linker-config"
out="$root/_build/configured-namespaces"
mkdir -p "$src" "$out"
expected=4838f33b8779f86b21a5970585b7e02f23becf40d93d518f84ed26df43f96108
if [[ ! -f "$src/linker_config.h" ]]; then
  staged="$(mktemp "$src/header.XXXXXX")"
  trap 'rm -f -- "$staged"' EXIT
  curl -fsSL 'https://android.googlesource.com/platform/bionic/+/09a271af557444c9a6b3f3146d6d474156fd6cdb/linker/linker_config.h?format=TEXT' | base64 -D > "$staged"
  [[ "$(shasum -a 256 "$staged" | awk '{print $1}')" == "$expected" ]]
  mv "$staged" "$src/linker_config.h"
fi
[[ "$(shasum -a 256 "$src/linker_config.h" | awk '{print $1}')" == "$expected" ]]
CARGO_TARGET_DIR="$root/target" cargo build --release --manifest-path "$root/Cargo.toml" -p darwin-art-runtime
xcrun clang++ -std=c++20 -arch arm64 -D__ANDROID_API__=36 '-D__INTRODUCED_IN(n)=' \
  -I"$src" -I"$root/_aosp/android16-native-loader-policy" \
  -I"$root/_aosp/system/libbase/include" -I"$root/include" -I"$root/compat" \
  "$root/compat/loader/configured_namespaces.cc" \
  "$root/compat/loader/namespace_handles.cc" \
  "$root/compat/loader/namespace_creation.cc" \
  "$root/compat/loader/namespace_images.cc" \
  "$root/tools/native-loader-policy/configured_namespaces_test.cc" \
  "$root/target/release/libdarwin_art_runtime.a" \
  -framework Security -framework CoreFoundation -liconv -Wl,-dead_strip -o "$out/test"
"$out/test"
