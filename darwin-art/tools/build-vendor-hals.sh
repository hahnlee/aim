#!/bin/bash
# Builds the vendor HAL services of the derived image (ADR 0012 decision 3,
# docs/host-call.md): Rust programs for aarch64-linux-android that use AOSP's
# Rust binder crate and AIDL-generated Rust code over the ORIGINAL image's
# libbinder_ndk.so, and reach their host side through host-call.
#
# Steps (all inputs pinned in hal/sources.lock and verified):
# 1. fetch AOSP libbinder_rs and the libbinder_ndk headers at the image's tag;
# 2. fetch each stable AIDL interface's frozen API, check its .hash, and
#    generate Rust with the SDK build-tools `aidl --lang=rust` against the
#    frozen APIs of its imports, plus the crate glue Soong's
#    aidl_rust_glue.py would write (tools/lib/vendor_hal_aidl.py);
# 3. cargo build the hal/ workspace with the NDK clang as linker, linking
#    libbinder_ndk.so from the image (bindgen runs on the NDK's libclang).
#
# The layout under _build/vendor-hals is fixed: hal/*/Cargo.toml point into
# it. Services land in _build/vendor-hals/bin, the sources image/overlay.toml
# names; driver libraries in _build/vendor-hals/lib.
#
# Usage: tools/build-vendor-hals.sh [--image DIR] [--ndk DIR] [--sdk DIR]
set -euo pipefail
trap 'echo "build-vendor-hals: failed at line $LINENO: $BASH_COMMAND" >&2' ERR

root="$(cd "$(dirname "$0")/.." && pwd)"
image="$root/_build/android16-image-full"
ndk=""
sdk="$HOME/Library/Android/sdk"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --image) image="$2"; shift 2 ;;
    --ndk) ndk="$2"; shift 2 ;;
    --sdk) sdk="$2"; shift 2 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done
if [[ -z "$ndk" ]]; then
  ndk="$(ls -d "$sdk"/ndk/* 2>/dev/null | sort -V | tail -1)"
fi
toolchain="$ndk/toolchains/llvm/prebuilt/darwin-x86_64"
[[ -x "$toolchain/bin/clang" ]] || { echo "Android NDK not found (pass --ndk)" >&2; exit 1; }
[[ -f "$image/system/lib64/libbinder_ndk.so" ]] || { echo "image missing: $image" >&2; exit 1; }
rustup target list --installed | grep -qx aarch64-linux-android || {
  echo "rust target missing: rustup target add aarch64-linux-android" >&2; exit 1; }

source "$root/hal/sources.lock"
# The generated code is the aidl compiler's output, so the compiler is
# pinned too.
aidl="$sdk/$AIDL_COMPILER"
[[ -x "$aidl" ]] || { echo "SDK $AIDL_COMPILER not found (pass --sdk)" >&2; exit 1; }
[[ "$(shasum -a 256 "$aidl" | cut -d' ' -f1)" == "$AIDL_COMPILER_SHA256" ]] || {
  echo "$aidl is not the pinned compiler (sha256 $AIDL_COMPILER_SHA256)" >&2; exit 1; }
out="$root/_build/vendor-hals"
src="$out/src"
downloads="$out/downloads"
mkdir -p "$src" "$downloads" "$out/bin" "$out/link"

tree_hash() {
  (cd "$1" && find . -type f ! -name .fetched -print0 | LC_ALL=C sort -z \
    | xargs -0 shasum -a 256) | shasum -a 256 | cut -d' ' -f1
}

# fetch PROJECT SUBTREE DEST: the subtree at AOSP_TAG, extracted into DEST.
fetch() {
  local project="$1" subtree="$2" dest="$3"
  [[ -e "$dest/.fetched" ]] && return 0
  local archive="$downloads/$(echo "$project-$subtree" | tr '/' '_').tar.gz"
  if [[ ! -s "$archive" ]]; then
    local code=""
    for _ in 1 2 3 4 5 6; do
      code="$(curl -sL -o "$archive.partial" -w '%{http_code}' \
        "https://android.googlesource.com/$project/+archive/refs/tags/$AOSP_TAG/$subtree.tar.gz")"
      [[ "$code" == 200 ]] && break
      sleep 5
    done
    [[ "$code" == 200 ]] || { echo "fetch failed: $project/$subtree ($code)" >&2; exit 1; }
    mv "$archive.partial" "$archive"
  fi
  rm -rf "$dest"
  mkdir -p "$dest"
  tar -xzf "$archive" -C "$dest"
  touch "$dest/.fetched"
}

# 1. Binder sources.
for entry in "${SOURCE_TREES[@]}"; do
  IFS='|' read -r project subtree want <<<"$entry"
  dest="$src/${project#platform/}/$subtree"
  fetch "$project" "$subtree" "$dest"
  got="$(tree_hash "$dest")"
  [[ "$got" == "$want" ]] || {
    echo "content hash mismatch: $project/$subtree: $got (lock: $want)" >&2; exit 1; }
done

# 2. AIDL interfaces. Every frozen API is fetched and checked first, since
# an interface is compiled against the frozen APIs of its imports.
for entry in "${AIDL_INTERFACES[@]}"; do
  IFS='|' read -r package version project api _ <<<"$entry"
  api_dir="$src/aidl_api/$package/$version"
  fetch "$project" "$api" "$api_dir"
  # Soong's check (aidlVerifyHashRule): sha1 of the sorted per-file sha1
  # lines and the previous version ("latest-version" for V1) against the
  # last line of .hash, which is also the hash compiled in.
  want="$(tail -1 "$api_dir/.hash")"
  previous=latest-version
  (( version > 1 )) && previous=$((version - 1))
  got="$( (cd "$api_dir" && find ./ -name '*.aidl' -print0 | LC_ALL=C sort -z \
    | xargs -0 shasum -a 1 && echo "$previous") | shasum -a 1 | cut -d' ' -f1)"
  [[ "$got" == "$want" ]] || { echo "$package V$version: API hash $got != $want" >&2; exit 1; }
done
python3 "$root/tools/lib/vendor_hal_aidl.py" "$aidl" "$src/aidl_api" "$out/aidl" \
  "$root/hal/aidl" "${AIDL_INTERFACES[@]}"

# 3. Build. libbinder_ndk.so and libnativewindow.so (whose LL-NDK calls the
# NDK stub lacks) are the image's own; libc, libdl, libm and
# liblog come from the NDK sysroot (the same LL-NDK ABI).
for lib in libbinder_ndk libnativewindow; do
  ln -sf "$image/system/lib64/$lib.so" "$out/link/$lib.so"
done
export ANDROID_NDK_HOME="$ndk"
export LIBCLANG_PATH="$toolchain/lib"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$toolchain/bin/aarch64-linux-android35-clang"
target_dir="${CARGO_TARGET_DIR:-$root/_build/vendor-hals/target}"
cargo build --manifest-path "$root/hal/Cargo.toml" --locked --release \
  --target aarch64-linux-android --target-dir "$target_dir" --workspace
# Each service package names its installed binary, and each driver package
# its installed library, in [package.metadata.vendor-hal].
mkdir -p "$out/lib"
(cd "$root/hal" && cargo metadata --no-deps --format-version 1) | python3 -c '
import json, sys
for p in json.load(sys.stdin)["packages"]:
    meta = (p["metadata"] or {}).get("vendor-hal", {})
    for key, kind, built in (("binary", "bin", "{}"), ("library", "cdylib", "lib{}.so")):
        if key in meta:
            [t] = [t["name"] for t in p["targets"] if kind in t["kind"]]
            print(built.format(t.replace("-", "_") if kind == "cdylib" else t),
                  "bin" if kind == "bin" else "lib", meta[key])' | while read -r built dir name; do
  install -m 0755 "$target_dir/aarch64-linux-android/release/$built" "$out/$dir/$name"
  echo "built $out/$dir/$name"
done
