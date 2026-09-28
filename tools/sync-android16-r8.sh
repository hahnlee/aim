#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
. "$ROOT/upstream/android16-r8.lock"
OUT="$ROOT/_prebuilt/android-16/tools/r8.jar"

verify() {
  [ -f "$OUT" ] &&
    [ "$(shasum -a 256 "$OUT" | awk '{print $1}')" = "$R8_JAR_SHA256" ]
}

if ! verify; then
  TMP=$(mktemp -d "${TMPDIR:-/tmp}/aim-r8.XXXXXX")
  trap 'rm -rf "$TMP"' EXIT HUP INT TERM
  mkdir -p "$(dirname -- "$OUT")"
  curl -fsSL --retry 3 \
    "https://android.googlesource.com/platform/prebuilts/r8/+/$R8_REVISION/r8.jar?format=TEXT" \
    -o "$TMP/r8.jar.b64"
  base64 -D -i "$TMP/r8.jar.b64" -o "$TMP/r8.jar"
  ACTUAL=$(shasum -a 256 "$TMP/r8.jar" | awk '{print $1}')
  [ "$ACTUAL" = "$R8_JAR_SHA256" ] || {
    echo "Android 16 R8 checksum mismatch: $ACTUAL" >&2
    exit 1
  }
  mv "$TMP/r8.jar" "$OUT"
fi

ACTUAL_VERSION=$(java -cp "$OUT" com.android.tools.r8.D8 --version)
case "$ACTUAL_VERSION" in
  "$R8_VERSION"*) ;;
  *) echo "Android 16 R8 version mismatch: $ACTUAL_VERSION" >&2; exit 1 ;;
esac
echo "sync-android16-r8: $R8_TAG $R8_REVISION $ACTUAL_VERSION"
