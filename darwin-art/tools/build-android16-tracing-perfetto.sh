#!/bin/bash
set -euo pipefail
export LC_ALL=C
root="$(cd "$(dirname "$0")/.." && pwd)"
source "$root/upstream/android16-tracing-perfetto.lock"
source_root="$root/_aosp/android16-tracing-perfetto"
out="$root/_build/tracing-perfetto"
angle="$root/_build/angle-source"
perfetto="$angle/third_party/perfetto"
mkdir -p "$source_root/include" "$out"
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
while read -r hash file; do
  fetch "$file" "https://android.googlesource.com/platform/frameworks/native/+/$FRAMEWORKS_NATIVE_REVISION/libs/tracing_perfetto/$file" "$hash"
done < "$root/upstream/android16-tracing-perfetto.sources"
fetch android_os_Trace.cpp "https://android.googlesource.com/platform/frameworks/base/+/$FRAMEWORKS_BASE_REVISION/core/jni/android_os_Trace.cpp" "$TRACE_JNI_SHA256"
patch_file="$root/patches/tracing-perfetto/0001-explicit-dynamic-track-name.patch"
[[ "$(shasum -a 256 "$patch_file" | awk '{print $1}')" == "$DYNAMIC_TRACK_NAME_PATCH_SHA256" ]]
mkdir -p "$out/source"
cp "$source_root/tracing_sdk.cpp" "$out/source/tracing_sdk.cpp"
patch --batch --forward -p1 -d "$out/source" < "$patch_file"
[[ "$(git -C "$perfetto" rev-parse HEAD)" == "$PERFETTO_REVISION" ]]
[[ "$(git -C "$angle" rev-parse HEAD)" == "$ANGLE_BUILD_ROOT_REVISION" ]]
git -C "$perfetto" diff --quiet HEAD --
if [[ "${1:-}" != --compile-framework-only ]]; then
  "$angle/buildtools/mac/gn" gen "$out/perfetto-out" --root="$angle" \
    --root-target=//third_party/perfetto/src/shared_lib:libperfetto_c \
    --args='is_debug=false is_component_build=false symbol_level=0 target_cpu="arm64" enable_perfetto_ipc=true'
  "$root/_aosp/external/skia/third_party/ninja/ninja" -j2 -C "$out/perfetto-out" libperfetto_c
fi
nativehelper="$root/_build/nativehelper-foundation/source/libnativehelper"
flags=( -std=c++23 -arch arm64 -fPIC -Wno-writable-strings
  -I"$source_root/include" -I"$perfetto/include"
  -I"$nativehelper/include_jni" -I"$nativehelper/include"
  -I"$nativehelper/include_platform" -I"$nativehelper/header_only_include"
  -I"$root/_aosp/system/logging/liblog/include"
  -I"$root/_aosp/system/core/libcutils/include"
  -I"$root/_aosp/system/core/libutils/include"
  -I"$root/_aosp/system/core/libsystem/include"
  -I"$root/_aosp/system/libbase/include" )
for unit in tracing_perfetto tracing_perfetto_internal tracing_sdk android_os_Trace; do
  input="$source_root/$unit.cpp"
  [[ "$unit" != tracing_sdk ]] || input="$out/source/tracing_sdk.cpp"
  xcrun clang++ "${flags[@]}" -c "$input" -o "$out/$unit.o"
done
xcrun libtool -static -o "$out/libandroid-tracing-perfetto-darwin.a" \
  "$out/tracing_perfetto.o" "$out/tracing_perfetto_internal.o" \
  "$out/tracing_sdk.o" "$out/android_os_Trace.o"
if [[ "${1:-}" != --compile-framework-only ]]; then
  install_name_tool -id @rpath/libperfetto_c.dylib "$out/perfetto-out/libperfetto_c.dylib"
  codesign --force --sign - --timestamp=none "$out/perfetto-out/libperfetto_c.dylib"
  xcrun clang++ "${flags[@]}" "$root/tools/tracing-perfetto-smoke.cc" \
    "$out/libandroid-tracing-perfetto-darwin.a" "$out/perfetto-out/libperfetto_c.dylib" \
    "$root/_build/graphics-foundations/libcutils-darwin.a" \
    "$root/_build/libbase-foundation/libandroid-base-darwin.a" \
    "$root/_build/graphics-foundations/liblog-darwin.a" \
    -Wl,-rpath,"$out/perfetto-out" -o "$out/smoke"
  "$out/smoke"
fi
