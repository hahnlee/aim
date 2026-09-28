#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
. "$ROOT/upstream/android16-google-smali.lock"
SOURCE="$ROOT/_aosp/google-smali"
OUT="$SOURCE/smali/build/libs/smali-$GOOGLE_SMALI_VERSION-fat.jar"

verify_source() {
  [ -d "$SOURCE/.git" ] &&
    [ "$(git -C "$SOURCE" rev-parse HEAD)" = "$GOOGLE_SMALI_REVISION" ]
}

verify_jar() {
  [ -f "$OUT" ] &&
    [ "$(shasum -a 256 "$OUT" | awk '{print $1}')" = "$GOOGLE_SMALI_FAT_JAR_SHA256" ]
}

if ! verify_source; then
  if [ -e "$SOURCE" ]; then
    echo "google-smali source exists at an unexpected revision: $SOURCE" >&2
    exit 1
  fi
  git clone https://android.googlesource.com/platform/external/google-smali "$SOURCE"
  git -C "$SOURCE" checkout --detach "$GOOGLE_SMALI_REVISION"
fi

if ! verify_jar; then
  (
    cd "$SOURCE"
    # The AOSP fork checks generated ANTLR/JFlex sources into src/main/java.
    # Disable the upstream regeneration tasks so Gradle does not compile the
    # same parser classes twice.
    ./gradlew --no-daemon :smali:clean :smali:fatJar \
      -x :smali:generateGrammarSource -x :smali:jflex
  )
fi

verify_jar || {
  echo "Android 16 google-smali checksum mismatch" >&2
  exit 1
}
ACTUAL_VERSION=$(java -jar "$OUT" --version | sed -n '1p')
[ "$ACTUAL_VERSION" = "smali $GOOGLE_SMALI_VERSION (http://smali.org)" ] || {
  echo "Android 16 google-smali version mismatch: $ACTUAL_VERSION" >&2
  exit 1
}
echo "sync-android16-smali: $GOOGLE_SMALI_TAG $GOOGLE_SMALI_REVISION $ACTUAL_VERSION"
