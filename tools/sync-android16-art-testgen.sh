#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
. "$ROOT/upstream/android16-art-testgen.lock"
OUT="$ROOT/_prebuilt/android-16/testgen/java.txt"

verify() {
  [ -f "$OUT" ] &&
    [ "$(shasum -a 256 "$OUT" | awk '{print $1}')" = \
      "$JAVA_COPYRIGHT_TEMPLATE_SHA256" ]
}

if ! verify; then
  TMP=$(mktemp -d "${TMPDIR:-/tmp}/aim-testgen.XXXXXX")
  trap 'rm -rf "$TMP"' EXIT HUP INT TERM
  mkdir -p "$(dirname -- "$OUT")"
  curl -fsSL --retry 3 \
    "https://android.googlesource.com/platform/development/+/$DEVELOPMENT_REVISION/docs/copyright-templates/java.txt?format=TEXT" \
    -o "$TMP/java.txt.b64"
  base64 -D -i "$TMP/java.txt.b64" -o "$TMP/java.txt"
  ACTUAL=$(shasum -a 256 "$TMP/java.txt" | awk '{print $1}')
  [ "$ACTUAL" = "$JAVA_COPYRIGHT_TEMPLATE_SHA256" ] || {
    echo "Android 16 testgen template checksum mismatch: $ACTUAL" >&2
    exit 1
  }
  mv "$TMP/java.txt" "$OUT"
fi

echo "sync-android16-art-testgen: $DEVELOPMENT_TAG $DEVELOPMENT_REVISION"
