#!/bin/bash
# Original compatibility-artifact producer test; not runtime policy acceptance.
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
[[ $# == 1 && "$1" = /* ]] || { echo "usage: $0 ABSOLUTE_PINNED_SYSTEM_IMAGE" >&2; exit 64; }
image="$1"
producer="$root/tools/prepare-android16-compat-config.sh"
manifest="$root/upstream/android16-system-compat-files.lock"
source "$root/tools/lib/system-compat-artifact.sh"
stage="$(mktemp -d "${TMPDIR:-/tmp}/system-compat-artifact-test.XXXXXX")"
cleanup() {
  chmod -R u+w "$stage" 2>/dev/null || true
  rm -rf -- "$stage"
}
trap cleanup EXIT
output="$stage/output"
bash "$producer" "$image" "$output"
darwin_art_verify_system_compat_config_inventory "$output" "$manifest"
[[ "$(find "$output/system/etc/compatconfig" -type f | wc -l | tr -d ' ')" == 10 ]]
[[ "$(find "$output/system/system_ext/etc/compatconfig" -type f | wc -l | tr -d ' ')" == 1 ]]
[[ -z "$(find "$output" -type f -perm +222 -print -quit)" ]]
if bash "$producer" "$image" "$output"; then
  echo 'existing output unexpectedly overwritten' >&2
  exit 1
fi
ln -s "$output" "$stage/output-link"
if bash "$producer" "$image" "$stage/output-link"; then
  echo 'symlink output unexpectedly accepted' >&2
  exit 1
fi
tampered="$stage/tampered"
cp -R "$output" "$tampered"
chmod -R u+w "$tampered"
rm "$tampered/system/etc/compatconfig/framework-platform-compat-config.xml"
ln -s "$output/system/etc/compatconfig/framework-platform-compat-config.xml" \
  "$tampered/system/etc/compatconfig/framework-platform-compat-config.xml"
if darwin_art_verify_system_compat_config_inventory "$tampered" "$manifest"; then
  echo 'symlinked compatibility XML unexpectedly accepted' >&2
  exit 1
fi
echo 'system compatibility artifact: exact inventory, hashes, read-only publication, overwrite/symlink rejection PASS'
