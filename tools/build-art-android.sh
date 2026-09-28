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
# Sources: the AOSP subtrees below, fetched at the image's tag
# (patches/art-android/sources.lock) into <out>/src.
#
# Usage: tools/build-art-android.sh [--image DIR] [--out DIR] [--ndk DIR]
#                                   [--xsdc DIR] [-- ninja args]
set -euo pipefail
trap 'echo "build-art-android: failed at line $LINENO: $BASH_COMMAND" >&2' ERR

root="$(cd "$(dirname "$0")/.." && pwd)"
image="$root/_build/android16-image-full"
out="$root/_build/art-android"
ndk=""
xsdc="$root/_build/xsdc"
java="/opt/homebrew/opt/openjdk@17/bin/java"
ninja_args=()
while [[ $# -gt 0 ]]; do
  case "$1" in
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
[[ -f "$image/apex/com.android.art/lib64/libart.so" ]] || { echo "image missing: $image" >&2; exit 1; }
if [[ ! -f "$xsdc/xsdc.jar" ]]; then
  bash "$root/tools/build-android16-xsdc.sh"
fi
[[ -x "$java" ]] || java="java"

source "$root/patches/art-android/sources.lock"
src="$out/src"
downloads="$out/downloads"
mkdir -p "$src" "$downloads"

# fetch PROJECT SUBTREE DEST [REVISION]: the subtree at the tag (or at
# REVISION), extracted into <src>/DEST.
fetch() {
  local project="$1" subtree="$2" dest="$3" revision="${4:-refs/tags/$ART_ANDROID_TAG}"
  [[ -e "$src/$dest/.fetched" ]] && return 0
  local archive="$downloads/$(echo "$project-$subtree" | tr '/' '_').tar.gz"
  if [[ ! -s "$archive" ]]; then
    local suffix="$revision"
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
# fetch_file PROJECT PATH DEST: one file (gitiles serves it base64-encoded).
fetch_file() {
  local project="$1" path="$2" dest="$src/$3"
  [[ -s "$dest" ]] && return 0
  mkdir -p "$(dirname "$dest")"
  local code=""
  for _ in 1 2 3 4 5 6; do
    code="$(curl -sL -o "$dest.base64" -w '%{http_code}' \
      "https://android.googlesource.com/$project/+/refs/tags/$ART_ANDROID_TAG/$path?format=TEXT")"
    [[ "$code" == 200 ]] && break
    sleep 5
  done
  [[ "$code" == 200 ]] || { echo "fetch failed: $project/$path ($code)" >&2; exit 1; }
  base64 -D -i "$dest.base64" -o "$dest.partial"
  rm "$dest.base64"
  mv "$dest.partial" "$dest"
}

# ART, staged below: only the subtrees the build reads.
art_subtrees=(build cmdline compiler dex2oat disassembler libartbase libartpalette
  libdexfile libelffile libnativebridge libnativeloader libprofile odrefresh
  openjdkjvm runtime sigchainlib tools/cpp-define-generator adbconnection
  dt_fd_forward/export)
for subtree in "${art_subtrees[@]}"; do
  fetch platform/art "$subtree" "art-upstream/$subtree"
done
fetch_file platform/art tools/generate_operator_out.py art-upstream/tools/generate_operator_out.py
# Headers and static pieces ART builds from source.
fetch platform/libcore ojluni/src/main/native libcore/ojluni/src/main/native
fetch platform/libnativehelper "" libnativehelper
fetch platform/system/libbase include system/libbase/include
fetch platform/system/libziparchive "" system/libziparchive
fetch platform/system/core libcutils/include system/core/libcutils/include
fetch platform/system/logging liblog/include system/logging/liblog/include
fetch platform/system/unwinding libunwindstack/include system/unwinding/libunwindstack/include
fetch platform/external/boringssl src external/boringssl/src
fetch platform/external/dlmalloc "" external/dlmalloc
fetch platform/external/fmtlib include external/fmtlib/include
fetch platform/external/googletest googletest/include external/googletest/googletest/include
fetch platform/external/lzma C external/lzma/C "$LZMA_REVISION"
fetch platform/external/tinyxml2 "" external/tinyxml2
fetch platform/external/vixl src external/vixl/src
fetch platform/external/zlib "" external/zlib
fetch platform/packages/modules/adb libs/adbconnection/include packages/modules/adb/libs/adbconnection/include
fetch platform/bionic libc/platform bionic/libc/platform
fetch platform/bionic libc/async_safe bionic/libc/async_safe
fetch platform/external/lz4 lib external/lz4/lib
fetch platform/external/cpu_features "" external/cpu_features
fetch platform/external/libcap "" external/libcap
fetch platform/external/perfetto src/profiling/memory/include external/perfetto/src/profiling/memory/include
fetch platform/frameworks/libs/modules-utils build frameworks/libs/modules-utils/build
fetch platform/packages/modules/StatsD lib/libstatssocket/include packages/modules/StatsD/lib/libstatssocket/include
fetch platform/packages/modules/StatsD lib/libstatspull/include packages/modules/StatsD/lib/libstatspull/include
fetch_file platform/system/apex apexd/ApexInfoList.xsd system/apex/apexd/ApexInfoList.xsd

# Stage ART: the fetched subtrees, then the series.
stage="$src/art"
series="$root/patches/art-android/series"
stamp="$src/.art-stage"
series_patches() {
  local patch
  while read -r patch; do
    if [[ -n "$patch" && "$patch" != \#* ]]; then echo "$patch"; fi
  done < "$series"
}
identity="$( (echo "${art_subtrees[*]}"; cd "$root/patches/art-android" &&
  shasum -a 256 series $(series_patches)) | shasum -a 256 | cut -c1-16)"
if [[ ! -f "$stamp" || "$(cat "$stamp")" != "$identity" ]]; then
  rm -rf "$stage"
  mkdir -p "$stage"
  rsync -a --exclude .fetched "$src/art-upstream/" "$stage/"
  for patch in $(series_patches); do
    patch --batch --forward --quiet -p1 -d "$stage" -i "$root/patches/art-android/$patch"
  done
  echo "$identity" > "$stamp"
fi

python3 "$root/tools/art-android/gen_build.py" \
  --art "$stage" --image "$image" --ndk "$ndk" --out "$out" \
  --xsdc "$xsdc" --apex-xsd "$src/system/apex/apexd/ApexInfoList.xsd" --java "$java"
ninja -C "$out" "${ninja_args[@]+"${ninja_args[@]}"}"
for arg in "${ninja_args[@]+"${ninja_args[@]}"}"; do
  [[ "$arg" == -n ]] && exit 0  # a dry run built nothing to strip
done

# The derived image (image/overlay.toml) takes copies without debug info;
# lib64/ and bin/ keep it for symbolization.
strip="$ndk/toolchains/llvm/prebuilt/darwin-x86_64/bin/llvm-strip"
mkdir -p "$out/stripped/lib64" "$out/stripped/bin"
for f in lib64/libartbase.so lib64/libdexfile.so lib64/libprofile.so lib64/libart.so \
         lib64/libopenjdkjvm.so lib64/libadbconnection.so bin/dex2oat64; do
  if [[ ! "$out/stripped/$f" -nt "$out/$f" ]]; then
    "$strip" --strip-debug -o "$out/stripped/$f" "$out/$f"
  fi
done
