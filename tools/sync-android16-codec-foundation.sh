#!/bin/bash
set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd)"
project_root="$(cd "$script_dir/.." && pwd)"
aosp="$project_root/_aosp"
lock_file="$project_root/upstream/android16-codec-foundation.lock"
source "$lock_file"

fetch_file() {
  local project="$1" revision="$2" relative="$3" destination="$4"
  [[ -f "$destination" ]] && return
  mkdir -p "$(dirname "$destination")"
  local temporary
  temporary="$(mktemp "${destination}.tmp.XXXXXX")"
  curl -fsSL "https://android.googlesource.com/$project/+/$revision/$relative?format=TEXT" \
    | python3 -c 'import base64,sys; sys.stdout.buffer.write(base64.b64decode(sys.stdin.buffer.read()))' \
    > "$temporary"
  mv "$temporary" "$destination"
}
fetch_subtree() {
  local project="$1" revision="$2" subtree="$3" destination="$4" marker="$5"
  [[ -f "$destination/$marker" ]] && return
  mkdir -p "$destination"
  curl -fsSL "https://android.googlesource.com/$project/+archive/$revision/$subtree.tar.gz" \
    | tar -xzf - -C "$destination"
}

image="$aosp/external/image_io"
fetch_file "$IMAGE_IO_PROJECT" "$IMAGE_IO_REVISION" Android.bp "$image/Android.bp"
fetch_subtree "$IMAGE_IO_PROJECT" "$IMAGE_IO_REVISION" includes "$image/includes" image_io/jpeg/jpeg_scanner.h
fetch_subtree "$IMAGE_IO_PROJECT" "$IMAGE_IO_REVISION" src "$image/src" jpeg/jpeg_scanner.cc

modp="$aosp/external/modp_b64"
for path in Android.bp modp_b64.cc modp_b64_data.h modp_b64/modp_b64.h; do
  fetch_file "$MODP_B64_PROJECT" "$MODP_B64_REVISION" "$path" "$modp/$path"
done

jpeg="$aosp/external/libjpeg-turbo"
fetch_file "$LIBJPEG_TURBO_PROJECT" "$LIBJPEG_TURBO_REVISION" Android.bp "$jpeg/Android.bp"
jpeg_selection="$project_root/_build/codec-foundation/jpeg-materialization.txt"
mkdir -p "$(dirname "$jpeg_selection")"
python3 - "$jpeg/Android.bp" > "$jpeg_selection" <<'PY'
import re, sys
from pathlib import Path
bp = Path(sys.argv[1]).read_text()
d = bp.index('name: "libjpeg-defaults"')
s = bp.index('srcs: [', d); e = bp.index('],', s)
sources = re.findall(r'"([^"]+)"', bp[s:e])
a = bp.index('arm64: {', e)
s = bp.index('srcs: [', a); e = bp.index('],', s)
sources += re.findall(r'"([^"]+)"', bp[s:e])
root_headers = '''cderror.h cdjpeg.h cmyk.h jchuff.h jconfig.h jconfigint.h
jdcoefct.h jdct.h jdhuff.h jdmainct.h jdmaster.h jdmerge.h jdsample.h
jerror.h jinclude.h jmemsys.h jmorecfg.h jpeg_nbits_table.h jpegcomp.h
jpegint.h jpeglib.h jpeglibmangler.h jsimd.h jsimddct.h jversion.h tjutil.h
transupp.h turbojpeg.h'''.split()
support = ['simd/jsimd.h', 'simd/arm/align.h', 'simd/arm/jchuff.h', 'simd/arm/neon-compat.h',
           'jccolext.c', 'jdcol565.c', 'jdcolext.c', 'jdmrg565.c', 'jdmrgext.c',
           'jstdhuff.c',
           'simd/arm/aarch64/jccolext-neon.c', 'simd/arm/jcgryext-neon.c',
           'simd/arm/jdcolext-neon.c', 'simd/arm/jdmrgext-neon.c']
print('\n'.join(dict.fromkeys(sources + root_headers + support)))
PY
jpeg_ready=1
while IFS= read -r path; do
  if [[ ! -f "$jpeg/$path" ]]; then jpeg_ready=0; break; fi
done < "$jpeg_selection"
if [[ "$jpeg_ready" -eq 0 ]]; then
  mkdir -p "$jpeg"
  curl -fsSL "https://android.googlesource.com/$LIBJPEG_TURBO_PROJECT/+archive/$LIBJPEG_TURBO_REVISION.tar.gz" \
    | tar -xzf - -C "$jpeg" -T "$jpeg_selection"
fi

ultra="$aosp/external/libultrahdr"
fetch_file "$LIBULTRAHDR_PROJECT" "$LIBULTRAHDR_REVISION" Android.bp "$ultra/Android.bp"
fetch_file "$LIBULTRAHDR_PROJECT" "$LIBULTRAHDR_REVISION" ultrahdr_api.h "$ultra/ultrahdr_api.h"
fetch_subtree "$LIBULTRAHDR_PROJECT" "$LIBULTRAHDR_REVISION" lib/include "$ultra/lib/include" ultrahdr/jpegr.h
ultra_paths=(
  lib/src/icc.cpp lib/src/jpegr.cpp lib/src/gainmapmath.cpp
  lib/src/gainmapmetadata.cpp lib/src/jpegrutils.cpp lib/src/multipictureformat.cpp
  lib/src/editorhelper.cpp lib/src/ultrahdr_api.cpp
  lib/src/dsp/arm/editorhelper_neon.cpp lib/src/dsp/arm/gainmapmath_neon.cpp
  lib/src/jpegencoderhelper.cpp lib/src/jpegdecoderhelper.cpp
)
ultra_missing=0
for path in "${ultra_paths[@]}"; do [[ -f "$ultra/$path" ]] || ultra_missing=1; done
if [[ "$ultra_missing" -ne 0 ]]; then
  mkdir -p "$ultra"
  curl -fsSL "https://android.googlesource.com/$LIBULTRAHDR_PROJECT/+archive/$LIBULTRAHDR_REVISION.tar.gz" \
    | tar -xzf - -C "$ultra" "${ultra_paths[@]}"
fi

echo "codec-foundation-sync: Android 16 sparse Gitless sources ready"
