#!/bin/bash
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
bash "$root/tools/materialize-android16-key-character-map.sh"
task_stage="$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-kcm-artifact.XXXXXX")"
trap 'rm -rf -- "$task_stage"' EXIT
task_stage="$(cd "$task_stage" && pwd -P)"
source_tree="$task_stage/source inputs"
artifact_tree="$task_stage/package inputs"
cp -R "$root/_aosp/android16-key-character-map" "$source_tree"

materialize() {
  ANDROID16_KCM_SOURCE_ROOT="$source_tree" \
  ANDROID16_KCM_ARTIFACT_ROOT="$artifact_tree" \
    bash "$root/tools/materialize-android16-key-character-map.sh"
}
materialize
before="$(stat -f '%m:%z:%Sp' "$artifact_tree/manifest")"
materialize
[[ "$(stat -f '%m:%z:%Sp' "$artifact_tree/manifest")" == "$before" ]]
source "$root/tools/lib/key-character-map-artifact.sh"
darwin_art_verify_key_character_map_inventory "$artifact_tree" \
  "$root/upstream/android16-key-character-map.lock"
for name in Generic Virtual; do
  source_file="$source_tree/frameworks/base/data/keyboards/$name.kcm"
  output_file="$artifact_tree/system/usr/keychars/$name.kcm"
  cmp "$source_file" "$output_file"
  [[ "$(stat -f '%Sp' "$output_file")" == '-r--r--r--' ]]
done
[[ ! -e "$artifact_tree/system/usr/keylayout/Generic.kcm" ]]

# Reject a caller-controlled alias before creating any artifact output.
ln -s "$source_tree" "$task_stage/source alias"
if ANDROID16_KCM_SOURCE_ROOT="$task_stage/source alias" \
   ANDROID16_KCM_ARTIFACT_ROOT="$task_stage/rejected output" \
   bash "$root/tools/materialize-android16-key-character-map.sh" \
   > "$task_stage/symlink.log" 2>&1; then
  echo 'key-character-map artifact: accepted a symlink ancestor' >&2
  exit 1
fi
rg -q 'symlink path component' "$task_stage/symlink.log"
[[ ! -e "$task_stage/rejected output" ]]
echo 'key-character-map artifact: PASS pinned inputs, space paths, repeat, read-only keychars, symlink rejection'
