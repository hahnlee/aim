#!/bin/bash
# Build-artifact provisioning for the pinned system partition packages and
# platform configuration (upstream/android16-system-partition.lock).
darwin_art_prepare_system_partition_artifact() (
  set -euo pipefail
  local root output image stage
  root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
  output="$root/_build/android16-system-partition"
  image="${DARWIN_ART_ANDROID16_SYSTEM_IMAGE:-$HOME/Library/Android/sdk/system-images/android-36/google_apis_playstore_ps16k/arm64-v8a/system.img}"
  if [[ ! -e "$output" && ! -L "$output" ]]; then
    source "$root/upstream/android16-linker-config.lock"
    [[ "$(shasum -a 256 "$image" | awk '{print $1}')" == "$SYSTEM_IMAGE_PS16K_SHA256" ]] || return 65
    stage="$(mktemp -d "$root/_build/.system-partition.XXXXXX")"
    python3 "$root/tools/bootclasspath/extract_system_partition.py" "$image" "$stage" \
      --lock "$root/upstream/android16-system-partition.lock" >&2 || {
      rm -rf -- "$stage"; return 65;
    }
    mv "$stage" "$output"
  fi
  darwin_art_verify_system_partition "$output"
  echo "$output"
)

darwin_art_verify_system_partition() (
  set -euo pipefail
  local input="$1" root identity path file
  root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
  [[ "$input" = /* && -d "$input" && ! -L "$input" ]] || return 64
  while read -r identity path extra; do
    [[ -z "${identity:-}" || "$identity" == \#* ]] && continue
    [[ -z "${extra:-}" && ( "$path" == /system/* || "$path" == /product/overlay/* ||
       "$path" == /vendor/etc/permissions/* || "$path" == /metadata/aconfig/* ) &&
       "$path" != *..* ]] || return 64
    file="$input$path"
    if [[ "$identity" == link:* ]]; then
      [[ -L "$file" && "$(readlink "$file")" == "${identity#link:}" ]] || return 65
    else
      [[ -f "$file" && ! -L "$file" &&
         "$(shasum -a 256 "$file" | awk '{print $1}')" == "$identity" ]] || {
        echo "system partition file identity mismatch: $file" >&2; return 65;
      }
    fi
  done < "$root/upstream/android16-system-partition.lock"
)

# Device paths (without the leading /) of every locked entry.
darwin_art_system_partition_entries() (
  set -euo pipefail
  local root
  root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
  awk '$1 !~ /^#/ && NF == 2 {sub("^/", "", $2); print $2}' \
    "$root/upstream/android16-system-partition.lock"
)
