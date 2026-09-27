#!/bin/bash
# Builds ART (libartbase, libdexfile, libprofile, libart) as android-arm64 ELF
# for the guest world of ADR 0012, with the base-relative reference series in
# patches/art-android/.
#
# Toolchain: the Android NDK clang (aarch64-linux-android), bionic sysroot
# headers, libc++ headers switched to the platform's `std::__1` ABI. Every
# shared library links against the ORIGINAL platform libraries of the pinned
# image (read-only), e.g. libbase, liblog, libartpalette, libc++, libc.
#
# Usage: tools/build-art-android.sh [--aosp DIR] [--image DIR] [--out DIR]
#                                   [--ndk DIR] [--xsdc DIR] [-- ninja args]
set -euo pipefail
trap 'echo "build-art-android: failed at line $LINENO: $BASH_COMMAND" >&2' ERR

root="$(cd "$(dirname "$0")/.." && pwd)"
aosp="$root/_aosp"
image="$root/_build/android16-image-full"
out="$root/_build/art-android"
ndk=""
xsdc="$root/_build/xsdc"
java="/opt/homebrew/opt/openjdk@17/bin/java"
ninja_args=()
while [[ $# -gt 0 ]]; do
  case "$1" in
    --aosp) aosp="$2"; shift 2 ;;
    --image) image="$2"; shift 2 ;;
    --out) out="$2"; shift 2 ;;
    --ndk) ndk="$2"; shift 2 ;;
    --xsdc) xsdc="$2"; shift 2 ;;
    --) shift; ninja_args=("$@"); break ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done
if [[ -z "$ndk" ]]; then
  ndk="$(ls -d "$HOME"/Library/Android/sdk/ndk/* 2>/dev/null | sort -V | tail -1)"
fi
[[ -x "$ndk/toolchains/llvm/prebuilt/darwin-x86_64/bin/clang" ]] || {
  echo "Android NDK not found (pass --ndk)" >&2; exit 1; }
[[ -f "$aosp/art/runtime/runtime.cc" ]] || { echo "pinned ART sources missing: $aosp/art" >&2; exit 1; }
[[ -f "$image/apex/com.android.art/lib64/libart.so" ]] || { echo "image missing: $image" >&2; exit 1; }
if [[ ! -f "$xsdc/xsdc.jar" ]]; then
  bash "$root/tools/build-android16-xsdc.sh"
fi
[[ -x "$java" ]] || java="java"

source "$root/patches/art-android/sources.lock"
src="$out/src"
downloads="$out/downloads"
mkdir -p "$src" "$downloads"

# Projects the pinned _aosp tree does not carry, at the same AOSP tag.
fetch() {
  local project="$1" subtree="$2" dest="$3"
  [[ -e "$src/$dest/.fetched" ]] && return 0
  local archive="$downloads/$(echo "$project-$subtree" | tr '/' '_').tar.gz"
  if [[ ! -s "$archive" ]]; then
    local suffix="refs/tags/$ART_ANDROID_TAG"
    [[ -n "$subtree" ]] && suffix="$suffix/$subtree"
    local code=""
    for _ in 1 2 3 4 5 6; do
      code="$(curl -sL -o "$archive.partial" -w '%{http_code}' \
        "https://android.googlesource.com/$project/+archive/$suffix.tar.gz")"
      [[ "$code" == 200 ]] && break
      sleep 5
    done
    [[ "$code" == 200 ]] || { echo "fetch failed: $project/$subtree ($code)" >&2; exit 1; }
    mv "$archive.partial" "$archive"
  fi
  rm -rf "${src:?}/$dest"
  mkdir -p "$src/$dest"
  tar -xzf "$archive" -C "$src/$dest"
  touch "$src/$dest/.fetched"
}
fetch platform/art build art-extra/build
fetch platform/art odrefresh art-extra/odrefresh
fetch platform/bionic libc/platform bionic/libc/platform
fetch platform/bionic libc/async_safe bionic/libc/async_safe
fetch platform/external/lz4 lib external/lz4/lib
fetch platform/external/cpu_features "" external/cpu_features
fetch platform/external/libcap "" external/libcap
fetch platform/external/perfetto src/profiling/memory/include external/perfetto/src/profiling/memory/include
fetch platform/frameworks/libs/modules-utils build frameworks/libs/modules-utils/build
fetch platform/packages/modules/StatsD lib/libstatssocket/include packages/modules/StatsD/lib/libstatssocket/include
fetch platform/packages/modules/StatsD lib/libstatspull/include packages/modules/StatsD/lib/libstatspull/include

# Stage ART: the pinned sources plus the fetched ART subtrees, then the series.
stage="$src/art"
series="$root/patches/art-android/series"
stamp="$src/.art-stage"
series_patches() {
  local patch
  while read -r patch; do
    if [[ -n "$patch" && "$patch" != \#* ]]; then echo "$patch"; fi
  done < "$series"
}
identity="$( (cd "$root/patches/art-android" && shasum -a 256 series $(series_patches)) | shasum -a 256 | cut -c1-16)"
if [[ ! -f "$stamp" || "$(cat "$stamp")" != "$identity" ]]; then
  rm -rf "$stage"
  mkdir -p "$stage"
  rsync -a "$aosp/art/" "$stage/"
  rsync -a "$src/art-extra/build/" "$stage/build/"
  rsync -a "$src/art-extra/odrefresh/" "$stage/odrefresh/"
  rm -f "$stage/build/.fetched" "$stage/odrefresh/.fetched"
  for patch in $(series_patches); do
    patch --batch --forward --quiet -p1 -d "$stage" -i "$root/patches/art-android/$patch"
  done
  echo "$identity" > "$stamp"
fi

python3 "$root/tools/art-android/gen_build.py" \
  --art "$stage" --aosp "$aosp" --image "$image" --ndk "$ndk" --out "$out" \
  --xsdc "$xsdc" --apex-xsd "$aosp/android16-linkerconfig-apex/ApexInfoList.xsd" --java "$java"
ninja -C "$out" "${ninja_args[@]+"${ninja_args[@]}"}"
