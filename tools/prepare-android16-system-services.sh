#!/bin/bash
# Materialize original system-server code only. No profile mutation, classpath
# activation, emulator feature advertisement or decompiled-source substitution.
set -euo pipefail
export LC_ALL=C TZ=UTC
root="$(cd "$(dirname "$0")/.." && pwd)"
[[ $# == 2 ]] || { echo "usage: $0 ABSOLUTE_SYSTEM_IMAGE NEW_ABSOLUTE_SERVICES_JAR" >&2; exit 64; }
image="$1"
output="$2"
[[ "$image" == /* && -f "$image" && ! -L "$image" &&
   "$output" == /* && "$output" != */../* && "$output" != */./* &&
   "${output##*/}" == services.jar && ! -e "$output" && ! -L "$output" ]] || {
  echo 'expected regular image and new absolute services.jar output' >&2; exit 64;
}
source "$root/upstream/android16-linker-config.lock"
source "$root/upstream/android16-system-services.lock"
actual="$(shasum -a 256 "$image")"
[[ "${actual%% *}" == "$SYSTEM_IMAGE_PS16K_SHA256" ]] || {
  echo 'system service source image hash mismatch' >&2; exit 65;
}
parent="${output%/*}"
mkdir -p "$parent"
stage="$(mktemp -d "$parent/.system-services.XXXXXX")"
trap 'rm -rf -- "$stage"' EXIT
CARGO_TARGET_DIR="$root/target" cargo build --quiet --release \
  --manifest-path "$root/tools/super-i18n-apex-extract/Cargo.toml"
"$root/target/release/super-i18n-apex-extract" \
  "$image" "$stage/services.jar" --path "$SYSTEM_SERVICES_PATH"
actual="$(shasum -a 256 "$stage/services.jar")"
[[ "${actual%% *}" == "$SYSTEM_SERVICES_SHA256" &&
   "$(stat -f %z "$stage/services.jar")" == "$SYSTEM_SERVICES_BYTES" ]] || {
  echo 'original system services payload mismatch' >&2; exit 65;
}
unzip -tqq "$stage/services.jar"
touch -t 200801010000 "$stage/services.jar"
chmod 444 "$stage/services.jar"
# Same-filesystem link publishes atomically without replacing an existing file.
# In particular, a racing producer cannot turn mv into a directory move.
ln "$stage/services.jar" "$output"
echo "original system services: verified=$output (not activated)"
