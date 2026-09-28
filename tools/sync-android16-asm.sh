#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
. "$ROOT/upstream/android16-asm.lock"
OUT="$ROOT/_prebuilt/android-16/tools/asm-$ASM_VERSION.jar"

verify() {
  [ -f "$OUT" ] &&
    [ "$(shasum -a 256 "$OUT" | awk '{print $1}')" = "$ASM_JAR_SHA256" ]
}

if ! verify; then
  TMP=$(mktemp -d "${TMPDIR:-/tmp}/aim-asm.XXXXXX")
  trap 'rm -rf "$TMP"' EXIT HUP INT TERM
  mkdir -p "$(dirname -- "$OUT")"
  curl -fsSL --retry 3 \
    "https://android.googlesource.com/platform/prebuilts/misc/+/$PREBUILTS_MISC_REVISION/common/asm/asm-$ASM_VERSION.jar?format=TEXT" \
    -o "$TMP/asm.jar.b64"
  base64 -D -i "$TMP/asm.jar.b64" -o "$TMP/asm.jar"
  ACTUAL=$(shasum -a 256 "$TMP/asm.jar" | awk '{print $1}')
  [ "$ACTUAL" = "$ASM_JAR_SHA256" ] || {
    echo "Android 16 ASM checksum mismatch: $ACTUAL" >&2
    exit 1
  }
  mv "$TMP/asm.jar" "$OUT"
fi

jar tf "$OUT" | grep -q '^org/objectweb/asm/ClassReader.class$'
echo "sync-android16-asm: $PREBUILTS_MISC_TAG $PREBUILTS_MISC_REVISION ASM $ASM_VERSION"
