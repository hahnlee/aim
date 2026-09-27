#!/bin/bash
# Regenerates the boot image for the ART exception (ADR 0012 decision 4,
# docs/art-exception-patches.md, #165): the image's own boot class path,
# compiled by the rebuilt dex2oat64 running on the Linux syscall layer
# (linux-run) with the original linker64, bionic and ART APEX libraries.
#
# Mirrors the image's boot image: the primary boot image for the BCP recorded
# in the original boot.oat (multi-image, speed-profile with the image's boot
# profiles, dirty-image-objects and preloaded-classes, at ART_BASE_ADDRESS),
# plus every mainline boot image extension the image ships (verify), compiled
# against it. The output is what image/overlay.toml replaces:
#   <out>/framework/arm64/boot*.{art,oat}  ->  /system/framework/arm64/
#   <out>/framework/boot*.vdex             ->  /system/framework/
#
# Usage: tools/build-art-boot-image.sh [--image DIR] [--art DIR] [--out DIR]
#                                      [--linux-run PATH]
set -euo pipefail
trap 'echo "build-art-boot-image: failed at line $LINENO: $BASH_COMMAND" >&2' ERR

root="$(cd "$(dirname "$0")/.." && pwd)"
image="$root/_build/android16-image-full"
art="$root/_build/art-android"
out=""
linux_run=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --image) image="$2"; shift 2 ;;
    --art) art="$2"; shift 2 ;;
    --out) out="$2"; shift 2 ;;
    --linux-run) linux_run="$2"; shift 2 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done
out="${out:-$art/boot-image}"
image="$(cd "$image" && pwd -P)"  # never a symlink: the tree is read-only input
framework="$image/system/framework"
[[ -f "$framework/arm64/boot.oat" ]] || { echo "image missing: $image" >&2; exit 1; }
[[ -x "$art/stripped/bin/dex2oat64" ]] || {
  echo "no ART exception build in $art (run tools/build-art-android.sh)" >&2; exit 1; }
if [[ -z "$linux_run" ]]; then
  cargo build --quiet --release --manifest-path "$root/Cargo.toml" -p darwin-linux-abi --bin linux-run
  linux_run="$(cargo metadata --format-version 1 --no-deps --manifest-path "$root/Cargo.toml" |
    python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')/release/linux-run"
fi

# A key of the original boot.oat's header (key\0value\0 pairs).
oat_key() {
  python3 - "$1" "$2" <<'EOF'
import sys
data, key = open(sys.argv[1], "rb").read(), sys.argv[2].encode() + b"\0"
at = data.index(key) + len(key)
print(data[at:data.index(b"\0", at)].decode())
EOF
}
bcp="$(oat_key "$framework/arm64/boot.oat" bootclasspath)"
IFS=: read -r -a jars <<< "$bcp"

# Mainline extensions: boot-<stem>.art files beyond the primary image.
primary=" "
for jar in "${jars[@]}"; do primary+="$(basename "$jar" .jar) "; done
extensions=()
for art_file in "$framework"/arm64/boot-*.art; do
  stem="$(basename "$art_file" .art)"; stem="${stem#boot-}"
  [[ "$primary" == *" $stem "* ]] && continue
  jar=""
  for candidate in "$image"/apex/*/javalib/"$stem".jar "$image/system/framework/$stem.jar"; do
    [[ -f "$candidate" ]] && jar="${candidate#"$image"}" && break
  done
  [[ -n "$jar" ]] || { echo "no jar for boot image extension $stem" >&2; exit 1; }
  extensions+=("$jar")
done

# The guest view: the original image with the ART exception binaries in
# place of the ART APEX's, and a writable /data and /tmp.
work="$out.work"
rm -rf "$work" "$out"
mkdir -p "$work/data/out/arm64" "$work/data/ext/arm64" "$work/tmp"
{
  printf 'root\t/\t%s\n' "$image"
  printf 'rw\t/data\t%s\n' "$work/data"
  printf 'rw\t/tmp\t%s\n' "$work/tmp"
  for lib in libart libartbase libdexfile libprofile libopenjdkjvm; do
    printf 'rw\t/apex/com.android.art/lib64/%s.so\t%s\n' "$lib" "$art/stripped/lib64/$lib.so"
  done
  printf 'rw\t/apex/com.android.art/bin/dex2oat64\t%s\n' "$art/stripped/bin/dex2oat64"
} > "$work/path-map"

# Without a generated linker configuration linker64 uses its default
# namespace, which LD_LIBRARY_PATH points at the APEXes libart links.
dex2oat() {
  env -i LD_LIBRARY_PATH=/apex/com.android.art/lib64:/apex/com.android.i18n/lib64:/apex/com.android.os.statsd/lib64 \
    ANDROID_ROOT=/system ANDROID_DATA=/data ANDROID_ART_ROOT=/apex/com.android.art \
    ANDROID_I18N_ROOT=/apex/com.android.i18n \
    "$linux_run" --inherit-env --path-map "$work/path-map" --cache "$work/cache" \
    /apex/com.android.art/bin/dex2oat64 \
    --runtime-arg -Xms64m --runtime-arg -Xmx1024m \
    --instruction-set=arm64 --instruction-set-features=default \
    --image-format=lz4 --force-determinism --generate-build-id --avoid-storing-invocation \
    --compilation-reason=prebuilt --android-root=/system "$@"
}

primary_args=()
for jar in "${jars[@]}"; do primary_args+=(--dex-file="$jar" --dex-location="$jar"); done
dex2oat "${primary_args[@]}" \
  --profile-file=/apex/com.android.art/etc/boot-image.prof \
  --profile-file=/system/etc/boot-image.prof \
  --dirty-image-objects=/apex/com.android.art/etc/dirty-image-objects \
  --dirty-image-objects=/system/etc/dirty-image-objects \
  --preloaded-classes=/system/etc/preloaded-classes \
  --compiler-filter=speed-profile --resolve-startup-const-strings=true \
  --abort-on-hard-verifier-error --base=0x70000000 \
  --oat-file=/data/out/arm64/boot.oat --oat-location=/system/framework/arm64/boot.oat \
  --image=/data/out/arm64/boot.art

# An extension's image is named after the base, "boot", plus its jar's stem.
for jar in "${extensions[@]}"; do
  dex2oat --dex-file="$jar" --dex-location="$jar" --compiler-filter=verify \
    --runtime-arg -Xbootclasspath:"$bcp:$jar" --runtime-arg -Xbootclasspath-locations:"$bcp:$jar" \
    --boot-image=/data/out/boot.art \
    --oat-file=/data/ext/arm64/boot.oat --oat-location=/system/framework/arm64/boot.oat \
    --image=/data/ext/arm64/boot.art
  mv "$work"/data/ext/arm64/* "$work/data/out/arm64/"
done

mkdir -p "$out/framework/arm64"
for f in "$work"/data/out/arm64/*.art "$work"/data/out/arm64/*.oat; do
  cp "$f" "$out/framework/arm64/"
done
cp "$work"/data/out/arm64/*.vdex "$out/framework/"
rm -rf "$work"
echo "boot image: $out ($(ls "$out/framework/arm64" | wc -l | tr -d ' ') files for ${#jars[@]} BCP jars + ${#extensions[@]} extension(s))"
