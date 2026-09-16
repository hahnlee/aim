#!/bin/bash
set -euo pipefail
project_root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$project_root"
source_root="$project_root/_aosp/system/core/libutils"
[[ "$(shasum -a 256 "$source_root/Looper.cpp" | awk '{print $1}')" == d20aa7c65c2004cd29eca5c325e2e62e8bc0376a632b3a5bbfb47089a8b87237 ]]
[[ "$(shasum -a 256 "$source_root/include/utils/Looper.h" | awk '{print $1}')" == cc0a51dc121406ee1d7b7b86c8fd105f3cd4ab488234cf57ecdf29161e2b01a0 ]]
out="$project_root/_build/looper-transport"
mkdir -p "$out/source/include/utils"
cp "$source_root/Looper.cpp" "$out/source/Looper.cpp"
cp "$source_root/include/utils/Looper.h" "$out/source/include/utils/Looper.h"
patch --batch --fuzz=0 -s -d "$out/source" -p1 < "$project_root/patches/libutils/darwin-looper-platform.patch"
shadow="$project_root/_build/surfaceflinger-core/work/frameworks-native"
generated="$project_root/_build/surfaceflinger-core/work/generated"
libhidl="$project_root/_aosp/android16-surfaceflinger-core/system-libhidl"
libfmq="$project_root/_aosp/android16-surfaceflinger-core/system-libfmq"
sdk_root="$(xcrun --sdk macosx --show-sdk-path)"
source "$project_root/tools/lib/surfaceflinger-compile-flags.sh"
xcrun clang++ -I"$out/source/include" -I"$project_root/compat/looper" "${flags[@]}" \
  -UANDROID_UTILS_REF_BASE_DISABLE_IMPLICIT_CONSTRUCTION \
  -c "$out/source/Looper.cpp" -o "$out/Looper.o"
echo "Original Looper Darwin boundary compiled (not linked or run)"
