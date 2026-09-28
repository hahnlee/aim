#!/bin/bash
# Builds the replaced native daemons of the derived image (ADR 0012
# appendix, "Replaced native daemons"): Rust programs for
# aarch64-linux-android that serve the platform's own binder interfaces
# (IApexService, IVold, INetd) over the ORIGINAL image's libbinder_ndk.so,
# with AOSP's Rust binder crate in its system (platform) variant.
#
# Steps (all inputs pinned in daemons/sources.lock and verified):
# 1. the binder sources of tools/build-vendor-hals.sh (run first when
#    missing), plus each AIDL source tree at the image's tag;
# 2. Rust from each interface with the SDK `aidl --lang=rust`
#    (tools/lib/daemon_aidl.py);
# 3. cargo build the daemons/ workspace, linking the image's
#    libbinder_ndk.so.
#
# Programs land in _build/daemons/bin, the sources image/overlay.toml names.
#
# Usage: tools/build-daemons.sh [--image DIR] [--ndk DIR] [--sdk DIR]
set -euo pipefail
trap 'echo "build-daemons: failed at line $LINENO: $BASH_COMMAND" >&2' ERR

root="$(cd "$(dirname "$0")/.." && pwd)"
image="$root/_build/android16-image-full"
ndk=""
sdk="$HOME/Library/Android/sdk"
args=("$@")
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

# 1. The Rust binder sources and the pinned aidl compiler are the vendor
# HAL pipeline's.
hals="$root/_build/vendor-hals"
if [[ ! -e "$hals/src/frameworks/native/libs/binder/rust/.fetched" ]]; then
  bash "$root/tools/build-vendor-hals.sh" "${args[@]+"${args[@]}"}"
fi
source "$root/hal/sources.lock"
aidl="$sdk/$AIDL_COMPILER"
[[ "$(shasum -a 256 "$aidl" | cut -d' ' -f1)" == "$AIDL_COMPILER_SHA256" ]] || {
  echo "$aidl is not the pinned compiler (sha256 $AIDL_COMPILER_SHA256)" >&2; exit 1; }
source "$root/daemons/sources.lock"
out="$root/_build/daemons"
src="$out/src"
downloads="$out/downloads"
mkdir -p "$src" "$downloads" "$out/bin"

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
    for _ in 1 2 3 4 5 6 7 8 9 10; do
      code="$(curl -sL -o "$archive.partial" -w '%{http_code}' \
        "https://android.googlesource.com/$project/+archive/refs/tags/$AOSP_TAG/$subtree.tar.gz")"
      [[ "$code" == 200 ]] && tar -tzf "$archive.partial" >/dev/null 2>&1 && break
      code=""
      sleep 5
    done
    [[ "$code" == 200 ]] || { echo "fetch failed: $project/$subtree" >&2; exit 1; }
    mv "$archive.partial" "$archive"
  fi
  rm -rf "$dest"
  mkdir -p "$dest"
  tar -xzf "$archive" -C "$dest"
  touch "$dest/.fetched"
}

for entry in "${SOURCE_TREES[@]}"; do
  IFS='|' read -r project subtree want <<<"$entry"
  dest="$src/${project#platform/}/$subtree"
  fetch "$project" "$subtree" "$dest"
  got="$(tree_hash "$dest")"
  [[ "$got" == "$want" ]] || {
    echo "content hash mismatch: $project/$subtree: $got (lock: $want)" >&2; exit 1; }
done

# 2. AIDL crates.
python3 "$root/tools/lib/daemon_aidl.py" "$aidl" "$src" "$out/aidl" "$root/daemons/aidl" \
  "${AIDL_CRATES[@]}"

# 3. Build, linking the image's libbinder_ndk.so (the vendor pipeline's
# link directory).
ln -sf "$image/system/lib64/libbinder_ndk.so" "$hals/link/libbinder_ndk.so"
export ANDROID_NDK_HOME="$ndk"
export LIBCLANG_PATH="$toolchain/lib"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$toolchain/bin/aarch64-linux-android35-clang"
target_dir="${CARGO_TARGET_DIR:-$out/target}"
cargo build --manifest-path "$root/daemons/Cargo.toml" --locked --release \
  --target aarch64-linux-android --target-dir "$target_dir" --workspace
# Each daemon package names its installed program in [package.metadata.daemon].
(cd "$root/daemons" && cargo metadata --no-deps --format-version 1) | python3 -c '
import json, sys
for p in json.load(sys.stdin)["packages"]:
    name = (p["metadata"] or {}).get("daemon", {}).get("program")
    if name:
        [t] = [t["name"] for t in p["targets"] if "bin" in t["kind"]]
        print(t, name)' | while read -r built name; do
  install -m 0755 "$target_dir/aarch64-linux-android/release/$built" "$out/bin/$name"
  echo "built $out/bin/$name"
done
