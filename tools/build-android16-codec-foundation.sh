#!/bin/bash
set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/.." && pwd)"
aosp="$project_root/_aosp"
build="$project_root/_build/codec-foundation"
objects="$build/objects"
lock_file="$project_root/upstream/android16-codec-foundation.lock"
source "$lock_file"

"$script_dir/sync-android16-codec-foundation.sh"
mkdir -p "$objects"

sha256() { shasum -a 256 "$1" | awk '{print $1}'; }
verify_hash() {
  local path="$1" expected="$2" actual
  [[ -f "$path" ]] || { echo "codec-foundation: missing $path" >&2; exit 2; }
  actual="$(sha256 "$path")"
  [[ "$actual" == "$expected" ]] || {
    echo "codec-foundation: identity mismatch: $path" >&2
    echo "expected=$expected actual=$actual" >&2
    exit 3
  }
}
make_manifest() {
  local root="$1" list="$2" output="$3"
  python3 - "$root" "$list" > "$output" <<'PY'
import hashlib, sys
from pathlib import Path
root = Path(sys.argv[1])
for relative in Path(sys.argv[2]).read_text().splitlines():
    data = (root / relative).read_bytes()
    print(f"{hashlib.sha256(data).hexdigest()}  {relative}")
PY
}
check_manifest() {
  local name="$1" root="$2" list="$3" expected_count="$4" expected_sha="$5"
  local output="$build/$name-source-manifest.txt" count actual
  make_manifest "$root" "$list" "$output"
  count="$(wc -l < "$list" | tr -d ' ')"
  actual="$(sha256 "$output")"
  [[ "$count" == "$expected_count" && "$actual" == "$expected_sha" ]] || {
    echo "codec-foundation: $name Android.bp selection changed" >&2
    echo "expected_count=$expected_count actual_count=$count" >&2
    echo "expected_sha=$expected_sha actual_sha=$actual" >&2
    exit 3
  }
}

image="$aosp/external/image_io"
modp="$aosp/external/modp_b64"
jpeg="$aosp/external/libjpeg-turbo"
ultra="$aosp/external/libultrahdr"
verify_hash "$image/Android.bp" "$IMAGE_IO_ANDROID_BP_SHA256"
verify_hash "$modp/Android.bp" "$MODP_B64_ANDROID_BP_SHA256"
verify_hash "$modp/modp_b64.cc" "$MODP_B64_SOURCE_SHA256"
verify_hash "$jpeg/Android.bp" "$LIBJPEG_TURBO_ANDROID_BP_SHA256"
verify_hash "$ultra/Android.bp" "$LIBULTRAHDR_ANDROID_BP_SHA256"

image_list="$build/image-io-sources.txt"
find "$image/src" -type f -name '*.cc' | sed "s#^$image/##" | LC_ALL=C sort > "$image_list"
check_manifest image-io "$image" "$image_list" "$IMAGE_IO_SOURCE_COUNT" "$IMAGE_IO_SOURCE_MANIFEST_SHA256"

jpeg_list="$build/libjpeg-sources.txt"
python3 - "$jpeg/Android.bp" > "$jpeg_list" <<'PY'
import re, sys
from pathlib import Path
bp = Path(sys.argv[1]).read_text()
d = bp.index('name: "libjpeg-defaults"')
s = bp.index('srcs: [', d); e = bp.index('],', s)
sources = re.findall(r'"([^"]+)"', bp[s:e])
a = bp.index('arm64: {', e)
s = bp.index('srcs: [', a); e = bp.index('],', s)
sources += re.findall(r'"([^"]+)"', bp[s:e])
print('\n'.join(sources))
PY
check_manifest libjpeg "$jpeg" "$jpeg_list" "$LIBJPEG_TURBO_SOURCE_COUNT" "$LIBJPEG_TURBO_SOURCE_MANIFEST_SHA256"

ultra_list="$build/libultrahdr-sources.txt"
printf '%s\n' \
  lib/src/icc.cpp lib/src/jpegr.cpp lib/src/gainmapmath.cpp \
  lib/src/gainmapmetadata.cpp lib/src/jpegrutils.cpp lib/src/multipictureformat.cpp \
  lib/src/editorhelper.cpp lib/src/ultrahdr_api.cpp \
  lib/src/dsp/arm/editorhelper_neon.cpp lib/src/dsp/arm/gainmapmath_neon.cpp \
  > "$ultra_list"
check_manifest libultrahdr "$ultra" "$ultra_list" "$LIBULTRAHDR_SOURCE_COUNT" "$LIBULTRAHDR_SOURCE_MANIFEST_SHA256"
printf '%s\n' lib/src/jpegencoderhelper.cpp > "$build/libjpegencoder-sources.txt"
check_manifest libjpegencoder "$ultra" "$build/libjpegencoder-sources.txt" 1 "$LIBJPEG_ENCODER_SOURCE_MANIFEST_SHA256"
printf '%s\n' lib/src/jpegdecoderhelper.cpp > "$build/libjpegdecoder-sources.txt"
check_manifest libjpegdecoder "$ultra" "$build/libjpegdecoder-sources.txt" 1 "$LIBJPEG_DECODER_SOURCE_MANIFEST_SHA256"

cxx="$(command -v clang++)"
cc="$(command -v clang)"
ar="$(xcrun --find ar)"
compile_archive() {
  local name="$1" compiler="$2" root="$3" list="$4"
  shift 4
  local object_list="$build/$name-objects.txt"
  : > "$object_list"
  while IFS= read -r relative; do
    local object="$objects/${name}_${relative//\//_}.o"
    object="${object%.*}.o"
    echo "codec-foundation: compile $name/$relative"
    "$compiler" "$@" -c "$root/$relative" -o "$object"
    printf '%s\n' "$object" >> "$object_list"
  done < "$list"
  local archive="$build/lib$name-darwin.a"
  rm -f "$archive"
  "$ar" rcs "$archive" $(<"$object_list")
}

compile_archive modpb64 "$cxx" "$modp" <(printf '%s\n' modp_b64.cc) \
  -std=c++17 -arch arm64 -fPIC -Wall -Werror -I"$modp" -I"$modp/modp_b64"
compile_archive image_io "$cxx" "$image" "$image_list" \
  -std=c++17 -arch arm64 -fPIC -fno-exceptions -frtti -DUNIX_ENV=1 \
  -Wall -Werror -Wno-reorder -Wno-unused-parameter -I"$image/includes" -I"$modp"
compile_archive jpeg "$cc" "$jpeg" "$jpeg_list" \
  -std=c11 -arch arm64 -fPIC -DWITH_SIMD -DNO_GETENV -DNEON_INTRINSICS \
  -O3 -fstrict-aliasing -Werror -Wno-sign-compare -Wno-unused-parameter \
  -I"$jpeg" -I"$jpeg/simd/arm"
ultra_flags=(
  -std=c++17 -arch arm64 -fPIC -frtti -DUHDR_ENABLE_INTRINSICS -DUHDR_WRITE_XMP
  -I"$ultra" -I"$ultra/lib/include" -I"$image/includes" -I"$jpeg"
)
compile_archive ultrahdr "$cxx" "$ultra" "$ultra_list" "${ultra_flags[@]}"
compile_archive jpegencoder "$cxx" "$ultra" "$build/libjpegencoder-sources.txt" "${ultra_flags[@]}"
compile_archive jpegdecoder "$cxx" "$ultra" "$build/libjpegdecoder-sources.txt" "${ultra_flags[@]}"

for item in 'modpb64 1' 'image_io 36' 'jpeg 67' 'ultrahdr 10' 'jpegencoder 1' 'jpegdecoder 1'; do
  set -- $item
  members="$($ar -t "$build/lib$1-darwin.a" | grep -v '^__\.SYMDEF' | wc -l | tr -d ' ')"
  [[ "$members" == "$2" ]] || { echo "codec-foundation: $1 members=$members expected=$2" >&2; exit 3; }
done

combined="$build/codec-foundation-force-loaded.o"
"$cxx" -r -arch arm64 \
  -Wl,-force_load,"$build/libmodpb64-darwin.a" \
  -Wl,-force_load,"$build/libimage_io-darwin.a" \
  -Wl,-force_load,"$build/libjpeg-darwin.a" \
  -Wl,-force_load,"$build/libjpegencoder-darwin.a" \
  -Wl,-force_load,"$build/libjpegdecoder-darwin.a" \
  -Wl,-force_load,"$build/libultrahdr-darwin.a" -o "$combined"
nm -u "$combined" | awk '$1 ~ /^_/ {print $1}' | sort -u > "$build/undefined-symbols.txt"
nm -gU "$combined" | awk '$2 ~ /^[Tt]$/ {print $3}' | sort -u > "$build/defined-symbols.txt"
for symbol in _modp_b64_decode _jpeg_CreateCompress _jpeg_CreateDecompress \
              _is_uhdr_image _uhdr_create_decoder; do
  grep -Fx "$symbol" "$build/defined-symbols.txt" >/dev/null || {
    echo "codec-foundation: required module definition missing: $symbol" >&2
    exit 3
  }
done
if grep -E '^_(jpeg_|uhdr_|is_uhdr_image|modp_b64_)' "$build/undefined-symbols.txt" >/dev/null; then
  echo "codec-foundation: unresolved symbol remains inside the codec module closure" >&2
  exit 3
fi
grep -F '__ZN22photos_editing_formats8image_io11JpegScanner3RunE' \
  "$build/defined-symbols.txt" >/dev/null || {
  echo "codec-foundation: ImageIO JpegScanner definition missing" >&2
  exit 3
}

smoke="$build/android16-codec-smoke"
"$cxx" -std=c++17 -arch arm64 "$project_root/probes/android16_codec_smoke.cpp" \
  -I"$jpeg" -I"$ultra" \
  -Wl,-force_load,"$build/libmodpb64-darwin.a" \
  -Wl,-force_load,"$build/libimage_io-darwin.a" \
  -Wl,-force_load,"$build/libjpeg-darwin.a" \
  -Wl,-force_load,"$build/libjpegencoder-darwin.a" \
  -Wl,-force_load,"$build/libjpegdecoder-darwin.a" \
  -Wl,-force_load,"$build/libultrahdr-darwin.a" -o "$smoke"
"$smoke" | tee "$build/smoke-output.txt"
echo "codec-foundation: archives=6 members=116 unresolved=$(wc -l < "$build/undefined-symbols.txt" | tr -d ' ') arm64=1"
