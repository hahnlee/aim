#!/bin/bash
# Build-artifact verification/provisioning for the original system_server
# classpath JARs (upstream/android16-systemserverclasspath.lock).
darwin_art_verify_systemserver_classpath() (
  set -euo pipefail
  local input="$1" root lock expected kind path file count=0
  root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
  lock="$root/upstream/android16-systemserverclasspath.lock"
  [[ "$input" = /* && -d "$input" && ! -L "$input" ]] || return 64
  while read -r expected kind path extra; do
    [[ -z "${expected:-}" || "$expected" == \#* || "$expected" == IMAGE_SHA256=* ]] && continue
    [[ -z "${extra:-}" && "$expected" =~ ^[0-9a-f]{64}$ &&
       ( "$kind" == classpath || "$kind" == standalone ) &&
       "$path" == /*.jar && "$path" != *..* ]] || return 64
    file="$input$path"
    [[ -f "$file" && ! -L "$file" ]] || { echo "missing system_server JAR: $file" >&2; return 69; }
    [[ "$(shasum -a 256 "$file" | awk '{print $1}')" == "$expected" ]] || {
      echo "system_server JAR identity mismatch: $file" >&2; return 65;
    }
    count=$((count + 1))
  done < "$lock"
  [[ "$count" -gt 0 ]] || return 64
)

# The derive_classpath environment for the listed KIND (classpath or
# standalone): device paths joined by ':' in lock order.
darwin_art_systemserver_classpath() (
  set -euo pipefail
  local want="$1" root
  root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
  awk -v want="$want" '$1 ~ /^[0-9a-f]+$/ && $2 == want {print $3}' \
    "$root/upstream/android16-systemserverclasspath.lock" | paste -sd: -
)

darwin_art_prepare_systemserver_classpath_artifact() (
  set -euo pipefail
  local root output image stage
  root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
  output="$root/_build/android16-systemserverclasspath-original"
  if [[ ! -e "$output" && ! -L "$output" ]]; then
    image="${DARWIN_ART_ANDROID16_SYSTEM_IMAGE:-$HOME/Library/Android/sdk/system-images/android-36/google_apis_playstore_ps16k/arm64-v8a/system.img}"
    source "$root/upstream/android16-linker-config.lock"
    [[ "$(shasum -a 256 "$image" | awk '{print $1}')" == "$SYSTEM_IMAGE_PS16K_SHA256" ]] || return 65
    stage="$(mktemp -d "$root/_build/.systemserverclasspath.XXXXXX")"
    python3 "$root/tools/bootclasspath/extract_systemserver.py" "$image" "$stage" >&2 || {
      rm -rf -- "$stage"; return 70;
    }
    darwin_art_verify_systemserver_classpath "$stage" || { rm -rf -- "$stage"; return 65; }
    mv "$stage" "$output"
  fi
  # A present but wrong artifact is an error, never silently replaced.
  darwin_art_verify_systemserver_classpath "$output"
  echo "$output"
)
