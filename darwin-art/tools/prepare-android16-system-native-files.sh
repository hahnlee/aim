#!/bin/bash
# Original guest files only; never substitutes host libraries or activates profiles.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
[[ $# == 1 && "$1" = /* ]] || { echo 'expected new absolute output directory' >&2; exit 2; }
out="$1"
[[ ! -e "$out" && ! -L "$out" ]] || { echo "destination exists: $out" >&2; exit 1; }
image="${DARWIN_ART_ANDROID16_SYSTEM_IMAGE:-$HOME/Library/Android/sdk/system-images/android-36/google_apis_playstore_ps16k/arm64-v8a/system.img}"
source "$root/upstream/android16-linker-config.lock"
[[ "$(shasum -a 256 "$image" | awk '{print $1}')" == "$SYSTEM_IMAGE_PS16K_SHA256" ]]
CARGO_TARGET_DIR="$root/target" cargo build --quiet --release --manifest-path "$root/tools/super-i18n-apex-extract/Cargo.toml"
mkdir -m 700 "$out"
while read -r expected guest_path; do
  [[ -n "$expected" && "$expected" != \#* ]] || continue
  [[ "$guest_path" == /system/lib64/* && "$guest_path" != *..* ]]
  destination="$out$guest_path"
  mkdir -p "${destination%/*}"
  "$root/target/release/super-i18n-apex-extract" "$image" "$destination" --path "$guest_path"
  [[ "$(shasum -a 256 "$destination" | awk '{print $1}')" == "$expected" ]]
done < "$root/upstream/android16-system-native-files.lock"
while read -r guest_path expected_target; do
  [[ -n "$guest_path" && "$guest_path" != \#* ]] || continue
  [[ "$guest_path" == /system/lib64/* && "$guest_path" != *..* ]]
  destination="$out$guest_path"
  mkdir -p "${destination%/*}"
  "$root/target/release/super-i18n-apex-extract" "$image" "$destination" --symlink "$guest_path"
  [[ "$(readlink "$destination")" == "$expected_target" ]]
done < "$root/upstream/android16-system-native-links.lock"
echo "Original system native files verified: $out (not activated)"
