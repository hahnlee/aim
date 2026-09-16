#!/bin/bash
# Build-artifact verification/provisioning only; no runtime feature policy.
darwin_art_verify_system_services() (
  set -euo pipefail
  local file="$1" root actual
  root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
  source "$root/upstream/android16-system-services.lock"
  [[ "$file" == /* && -f "$file" && ! -L "$file" ]] || {
    echo "missing original system service artifact: $file" >&2; return 69;
  }
  actual="$(shasum -a 256 "$file")"
  [[ "${actual%% *}" == "$SYSTEM_SERVICES_SHA256" &&
     "$(stat -f %z "$file")" == "$SYSTEM_SERVICES_BYTES" ]] || {
    echo "system service artifact identity mismatch: $file" >&2; return 65;
  }
)

darwin_art_prepare_system_services_artifact() (
  set -euo pipefail
  local root output image
  root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
  output="$root/_build/android16-system-services/services.jar"
  if [[ ! -e "$output" && ! -L "$output" ]]; then
    image="${DARWIN_ART_ANDROID16_SYSTEM_IMAGE:-$HOME/Library/Android/sdk/system-images/android-36/google_apis_playstore_ps16k/arm64-v8a/system.img}"
    bash "$root/tools/prepare-android16-system-services.sh" "$image" "$output" >&2
  fi
  # A present but wrong/symlink artifact is an error, never silently replaced.
  darwin_art_verify_system_services "$output"
  echo "$output"
)
