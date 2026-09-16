#!/bin/bash
# Extract original configuration inputs without copying/mounting system.img.
set -euo pipefail
export LC_ALL=C
root="$(cd "$(dirname "$0")/.." && pwd)"
source "$root/upstream/android16-linker-config.lock"
input="${1:-$HOME/Library/Android/sdk/system-images/android-36/google_apis_playstore_ps16k/arm64-v8a/system.img}"
[[ -f "$input" ]] || { echo "missing system image: $input" >&2; exit 1; }
image_sha="$(shasum -a 256 "$input" | awk '{print $1}')"
case "$image_sha" in
  "$SYSTEM_IMAGE_PS16K_SHA256") variant=ps16k ;;
  "$SYSTEM_IMAGE_SHA256") variant=4k ;;
  *) echo 'Unsupported system image identity; do not infer linker policy from another image' >&2; exit 1 ;;
esac
echo "linker configuration input image=$variant sha256=$image_sha"
CARGO_TARGET_DIR="$root/target" cargo build --quiet --release \
  --manifest-path "$root/tools/super-i18n-apex-extract/Cargo.toml"
# Use the exact output directory of the build above, never a stale second tool.
extractor="$root/target/release/super-i18n-apex-extract"
out="$root/_build/android16-linker-config/system/etc"
mkdir -p "$out"
stage="$(mktemp -d "$out/extract.XXXXXX")"
cleanup() {
  rm -f -- "$stage/public.libraries.txt" "$stage/linker.config.pb"
  rmdir -- "$stage"
}
trap cleanup EXIT
for name in public.libraries.txt linker.config.pb; do
  case "$name" in
    public.libraries.txt) expected="$PUBLIC_LIBRARIES_SHA256" ;;
    linker.config.pb) expected="$LINKER_CONFIG_PROTO_SHA256" ;;
  esac
  if [[ ! -e "$out/$name" ]]; then
    "$extractor" "$input" "$stage/$name" --path "/system/etc/$name"
    [[ "$(shasum -a 256 "$stage/$name" | awk '{print $1}')" == "$expected" ]]
    chmod 0400 "$stage/$name"
    mv -n "$stage/$name" "$out/$name"
  fi
  [[ "$(shasum -a 256 "$out/$name" | awk '{print $1}')" == "$expected" ]] || {
    echo "Configuration output identity mismatch: $out/$name" >&2; exit 1;
  }
done
echo "linker configuration inputs PASS: $out (generation/startup not performed)"
