#!/bin/bash
# Verify and prepare the pinned Android key-character-map resource artifact.
# This owns only the immutable /system/usr/keychars packaging input.

darwin_art_verify_key_character_map_inventory() (
  set -euo pipefail
  local input="$1" manifest="$2" actual expected name
  [[ "$input" = /* && -d "$input" && ! -L "$input" &&
     "$manifest" = /* && -f "$manifest" && ! -L "$manifest" ]] || return 64
  # shellcheck disable=SC1090
  source "$manifest"
  [[ "${KCM_RESOURCE_INSTALL_ROOT:-}" == system/usr/keychars &&
     "${KCM_RESOURCE_GENERIC_NAME:-}" == Generic.kcm &&
     "${KCM_RESOURCE_VIRTUAL_NAME:-}" == Virtual.kcm ]] || return 64
  for expected in \
      "$input/system" "$input/system/usr" "$input/system/usr/keychars"; do
    [[ -d "$expected" && ! -L "$expected" ]] || return 69
  done
  for name in "$KCM_RESOURCE_GENERIC_NAME" "$KCM_RESOURCE_VIRTUAL_NAME"; do
    actual="$input/$KCM_RESOURCE_INSTALL_ROOT/$name"
    [[ -f "$actual" && ! -L "$actual" ]] || return 69
  done
  [[ "$(shasum -a 256 "$input/$KCM_RESOURCE_INSTALL_ROOT/$KCM_RESOURCE_GENERIC_NAME")" == "$KCM_GENERIC_SHA256  $input/$KCM_RESOURCE_INSTALL_ROOT/$KCM_RESOURCE_GENERIC_NAME" ]] || return 65
  [[ "$(shasum -a 256 "$input/$KCM_RESOURCE_INSTALL_ROOT/$KCM_RESOURCE_VIRTUAL_NAME")" == "$KCM_VIRTUAL_SHA256  $input/$KCM_RESOURCE_INSTALL_ROOT/$KCM_RESOURCE_VIRTUAL_NAME" ]] || return 65

  local expected_names actual_names path
  expected_names="$(mktemp "${TMPDIR:-/tmp}/keychars-expected.XXXXXX")"
  actual_names="$(mktemp "${TMPDIR:-/tmp}/keychars-actual.XXXXXX")"
  trap 'rm -f -- "$expected_names" "$actual_names"' EXIT
  printf '%s\n' "$KCM_RESOURCE_GENERIC_NAME" "$KCM_RESOURCE_VIRTUAL_NAME" |
    sort > "$expected_names"
  for path in "$input/$KCM_RESOURCE_INSTALL_ROOT"/* \
              "$input/$KCM_RESOURCE_INSTALL_ROOT"/.[!.]* \
              "$input/$KCM_RESOURCE_INSTALL_ROOT"/..?*; do
    [[ -e "$path" || -L "$path" ]] || continue
    [[ -f "$path" && ! -L "$path" ]] || return 65
    printf '%s\n' "${path##*/}" >> "$actual_names"
  done
  sort -u "$actual_names" -o "$actual_names"
  cmp "$expected_names" "$actual_names" || return 65
)

darwin_art_prepare_key_character_map_artifact() (
  set -euo pipefail
  local root output
  root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
  output="${DARWIN_ART_ANDROID16_KCM_ROOT:-$root/_build/android16-key-character-map}"
  [[ "$output" = /* && "$output" != / && ! -L "$output" ]] || return 64
  if [[ -e "$output" ]]; then
    darwin_art_verify_key_character_map_inventory "$output" \
      "$root/upstream/android16-key-character-map.lock" || return
    echo "$output"
    return 0
  fi
  mkdir -p "${output%/*}"
  ANDROID16_KCM_ARTIFACT_ROOT="$output" \
    bash "$root/tools/materialize-android16-key-character-map.sh" >&2
  darwin_art_verify_key_character_map_inventory "$output" \
    "$root/upstream/android16-key-character-map.lock" || return
  echo "$output"
)
