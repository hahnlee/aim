#!/bin/bash
# Public libjnigraphics ABI gate. Symbol presence is necessary, not functional
# acceptance or permission to publish an incomplete native namespace image.
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
source "$root/upstream/android16-ndk-image-decoder.lock"
map="$root/_aosp/frameworks/base/native/graphics/jni/libjnigraphics.map.txt"
library="$root/_build/runtime-graphics-link-probe/libdarwin_art_runtime_graphics.dylib"
[[ "$(shasum -a 256 "$map" | awk '{print $1}')" == "$NDK_JNIGRAPHICS_MAP_SHA256" ]]
stage="$(mktemp -d "${TMPDIR:-/tmp}/ndk-graphics-exports.XXXXXX")"
trap 'rm -rf -- "$stage"' EXIT
awk '/^[[:space:]]+(AImageDecoder|AndroidBitmap)[A-Za-z0-9_]*;/ {
  sub(/;.*/, "", $1); print "_" $1
}' "$map" | LC_ALL=C sort -u > "$stage/required"
[[ "$(wc -l < "$stage/required" | tr -d ' ')" == 39 ]]
sed -nE 's/^[[:space:]]+PUBLIC_NDK\(([A-Za-z0-9_]+)\);$/_\1/p' \
  "$root/compat/loader/graphics_ndk_symbols.cc" | LC_ALL=C sort > "$stage/dispatch"
diff -u "$stage/required" "$stage/dispatch"
nm -gU "$library" | awk '$2 == "T" {print $3}' | LC_ALL=C sort -u > "$stage/actual"
LC_ALL=C comm -23 "$stage/required" "$stage/actual" > "$stage/missing"
if [[ -s "$stage/missing" ]]; then
  echo "libjnigraphics public ABI incomplete: missing $(wc -l < "$stage/missing" | tr -d ' ')/39" >&2
  cat "$stage/missing" >&2
  exit 1
fi
echo "libjnigraphics public ABI: 39 original-map exports and exact namespace dispatch (execution not tested)"
