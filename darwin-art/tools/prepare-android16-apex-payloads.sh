#!/bin/bash
# Offline original payload inputs. Does not publish an active APEX inventory.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
[[ $# == 1 && "$1" = /* ]] || { echo "usage: $0 NEW_ABSOLUTE_BUILD_DIRECTORY" >&2; exit 2; }
out="$1"
[[ ! -e "$out" && ! -L "$out" ]] || { echo "destination already exists: $out" >&2; exit 1; }
image="${DARWIN_ART_ANDROID16_SYSTEM_IMAGE:-$HOME/Library/Android/sdk/system-images/android-36/google_apis_playstore_ps16k/arm64-v8a/system.img}"
source "$root/upstream/android16-linker-config.lock"
[[ "$(shasum -a 256 "$image" | awk '{print $1}')" == "$SYSTEM_IMAGE_PS16K_SHA256" ]]
# Build serially; explicit target directory avoids another artifact tree.
CARGO_TARGET_DIR="$root/target" cargo build --release --manifest-path "$root/Cargo.toml" \
  -p apex-ext2-extract -p super-i18n-apex-extract
mkdir -m 700 "$out"
mkdir "$out/payloads" "$out/metadata" "$out/sources"
while read -r module expected relative; do
  [[ -n "$module" && "$module" != \#* ]] || continue
  archive="$root/$relative"
  if [[ "$relative" == SYSTEM_IMAGE ]]; then
    archive="$out/sources/$module.apex"
    "$root/target/release/super-i18n-apex-extract" "$image" "$archive" \
      --path "/system/apex/$module.apex"
  elif [[ "$relative" == SYSTEM_IMAGE_APEX:* ]]; then
    archive="$out/sources/$module.apex"
    "$root/target/release/super-i18n-apex-extract" "$image" "$archive" \
      --path "${relative#SYSTEM_IMAGE_APEX:}"
  elif [[ "$relative" == SYSTEM_IMAGE_CAPEX:* ]]; then
    device_path="${relative#SYSTEM_IMAGE_CAPEX:}"
    compressed="$out/sources/$module.capex"
    archive="$out/sources/$module.apex"
    "$root/target/release/super-i18n-apex-extract" "$image" "$compressed" \
      --path "$device_path"
    unzip -p "$compressed" original_apex > "$archive"
  fi
  [[ "$(shasum -a 256 "$archive" | awk '{print $1}')" == "$expected" ]]
  "$root/target/release/apex-ext2-extract" "$archive" "$out/payloads/$module" / --tree
  "$root/target/release/apex-ext2-extract" "$archive" - / --inventory > "$out/metadata/$module.inventory"
  source_record="$archive"
  if [[ "$archive" == "$out/"* ]]; then
    source_record="${archive#"$out/"}"
  fi
  printf '%s\t%s\t%s\n' "$module" "$expected" "$source_record" >> "$out/metadata/sources.tsv"
done < "$root/upstream/android16-apex-payloads.lock"
# A failed command leaves no completion receipt and never activates partial data.
cp "$root/upstream/android16-apex-payloads.lock" "$out/metadata/payloads.lock"
printf '%s\n' 'offline-payloads-v1; not activated' > "$out/metadata/complete"
echo "Original APEX payloads prepared: $out (not activated)"
