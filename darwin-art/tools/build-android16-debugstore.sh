#!/bin/bash
set -euo pipefail
export LC_ALL=C
root="$(cd "$(dirname "$0")/.." && pwd)"
source "$root/upstream/android16-debugstore.lock"
source_root="$root/_aosp/android16-debugstore"
out="$root/_build/debugstore"
mkdir -p "$source_root" "$out/crate/src" "$out/include/debugstore"
fetch() {
  local file="$1" url="$2" expected="$3"
  if [[ ! -f "$source_root/$file" ]]; then
    local staged
    staged="$(mktemp "$source_root/download.XXXXXX")"
    curl -fsSL "$url?format=TEXT" | base64 -D > "$staged"
    [[ "$(shasum -a 256 "$staged" | awk '{print $1}')" == "$expected" ]]
    mv "$staged" "$source_root/$file"
  fi
  [[ "$(shasum -a 256 "$source_root/$file" | awk '{print $1}')" == "$expected" ]]
}
for unit in lib core event event_type storage; do
  key="$(printf '%s_SHA256' "$unit" | tr '[:lower:]' '[:upper:]')"
  fetch "$unit.rs" "https://android.googlesource.com/platform/frameworks/native/+/$FRAMEWORKS_NATIVE_REVISION/libs/debugstore/rust/src/$unit.rs" "${!key}"
  cp "$source_root/$unit.rs" "$out/crate/src/$unit.rs"
done
fetch com_android_internal_os_DebugStore.cpp "https://android.googlesource.com/platform/frameworks/base/+/$FRAMEWORKS_BASE_REVISION/core/jni/com_android_internal_os_DebugStore.cpp" "$JNI_SHA256"
cp "$root/tools/debugstore/Cargo.toml" "$root/tools/debugstore/Cargo.lock" "$root/tools/debugstore/build.rs" "$out/crate/"
export DARWIN_ART_SOURCE_ROOT="$root"
export CARGO_TARGET_DIR="$out/target"
export MACOSX_DEPLOYMENT_TARGET=26.0
cargo build --locked --release --manifest-path "$out/crate/Cargo.toml"
cp "$out/target/cxxbridge/darwin-art-debugstore/src/lib.rs.h" "$out/include/debugstore/debugstore_cxx_bridge.rs.h"
nativehelper="$root/_build/nativehelper-foundation/source/libnativehelper"
helpers="$root/_aosp/frameworks-base-android-util-log/core/jni"
flags=( -std=c++20 -arch arm64 -fPIC -Wno-writable-strings
  -I"$out/include" -I"$out/target/cxxbridge"
  -I"$helpers" -I"$helpers/include"
  -I"$nativehelper/include_jni" -I"$nativehelper/include"
  -I"$nativehelper/include_platform" -I"$nativehelper/header_only_include"
  -I"$root/_aosp/system/logging/liblog/include"
  -I"$root/_aosp/system/core/libutils/include"
  -I"$root/_aosp/system/core/libsystem/include"
  -I"$root/_aosp/system/libbase/include" )
xcrun clang++ "${flags[@]}" -c "$source_root/com_android_internal_os_DebugStore.cpp" -o "$out/jni.o"
xcrun libtool -static -o "$out/libdebugstore-darwin.a" "$out/jni.o" "$out/target/release/libdarwin_art_debugstore.a"
xcrun clang++ "${flags[@]}" "$root/tools/debugstore/smoke.cc" "$out/target/release/libdarwin_art_debugstore.a" \
  "$root/_build/graphics-foundations/libutils-darwin.a" "$root/_build/graphics-foundations/liblog-darwin.a" -o "$out/smoke"
"$out/smoke"
