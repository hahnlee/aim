#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
. "$ROOT/upstream/android16-jasmin.lock"
OUT="$ROOT/_prebuilt/android-16/tools/jasmin.jar"

verify() {
  [ -f "$OUT" ] &&
    [ "$(shasum -a 256 "$OUT" | awk '{print $1}')" = "$JASMIN_JAR_SHA256" ]
}

if ! verify; then
  TMP=$(mktemp -d "${TMPDIR:-/tmp}/aim-jasmin.XXXXXX")
  trap 'rm -rf "$TMP"' EXIT HUP INT TERM
  mkdir -p "$(dirname -- "$OUT")"
  curl -fsSL --retry 3 \
    "https://android.googlesource.com/platform/dalvik/+/$DALVIK_REVISION/dx/etc/jasmin.jar?format=TEXT" \
    -o "$TMP/jasmin.jar.b64"
  base64 -D -i "$TMP/jasmin.jar.b64" -o "$TMP/jasmin.jar"
  ACTUAL=$(shasum -a 256 "$TMP/jasmin.jar" | awk '{print $1}')
  [ "$ACTUAL" = "$JASMIN_JAR_SHA256" ] || {
    echo "Android 16 Jasmin checksum mismatch: $ACTUAL" >&2
    exit 1
  }
  mv "$TMP/jasmin.jar" "$OUT"
fi

ACTUAL_VERSION=$(java -jar "$OUT" -version 2>&1)
case "$ACTUAL_VERSION" in
  *"$JASMIN_VERSION"*) ;;
  *) echo "Android 16 Jasmin version mismatch: $ACTUAL_VERSION" >&2; exit 1 ;;
esac
echo "sync-android16-jasmin: $DALVIK_TAG $DALVIK_REVISION jasmin $JASMIN_VERSION"
