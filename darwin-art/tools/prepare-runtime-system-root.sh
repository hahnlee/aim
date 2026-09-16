#!/bin/bash
# Build-time snapshot selection. Stdout is the selected path; diagnostics use
# stderr. This is not runtime profile selection or a library support claim.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
source "$root/tools/lib/system-native-inventory.sh"
[[ $# == 0 ]] || exit 64
cd "$root"
inputs=(
  tools/prepare-runtime-system-root.sh tools/prepare-android16-linker-root.sh
  tools/prepare-android16-system-native-files.sh tools/extract-android16-linker-config.sh
  tools/lib/system-native-inventory.sh
  upstream/android16-system-native-files.lock upstream/android16-system-native-links.lock
  upstream/android16-linker-config.lock upstream/android16-apex-payloads.lock
  _build/android16-apex-payloads/metadata/payloads.lock
  _build/android16-apex-payloads/metadata/sources.tsv
  _build/linkerconfig-apex/apex-inventory _build/linkerconfig-host/linkerconfig
)
recipe="$(shasum -a 256 "${inputs[@]}" | shasum -a 256)"
recipe="${recipe%% *}"
cache="$root/_build/runtime-system-roots"
selected="$cache/$recipe"
if [[ -e "$selected" || -L "$selected" ]]; then
  [[ -d "$selected" && ! -L "$selected" && -f "$selected/linkerconfig/ld.config.txt" ]] || exit 69
  darwin_art_verify_system_native_inventory "$selected" "$root/upstream/android16-system-native-files.lock"
  printf '%s\n' "$selected"
  exit 0
fi
mkdir -p "$cache"
stage="$(mktemp -d "$cache/.prepare.XXXXXX")"
trap 'rm -rf -- "$stage"' EXIT
bash "$root/tools/prepare-android16-linker-root.sh" "$stage/root" >&2
darwin_art_verify_system_native_inventory "$stage/root" "$root/upstream/android16-system-native-files.lock"
# Shared build orchestration serializes this producer; never merge into an
# existing directory if another producer nevertheless published it.
[[ ! -e "$selected" && ! -L "$selected" ]] || exit 73
mv "$stage/root" "$selected"
printf '%s\n' "$selected"
