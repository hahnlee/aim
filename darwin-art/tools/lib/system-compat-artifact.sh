#!/bin/bash
# Verify original compatibility XMLs; this is an immutable build artifact,
# not runtime policy or an active profile.
darwin_art_verify_system_compat_config_inventory() (
  set -euo pipefail
  local input="$1" manifest="$2" expected partition reader archive actual count
  [[ "$input" = /* && -d "$input" && ! -L "$input" &&
     "$manifest" = /* && -f "$manifest" && ! -L "$manifest" ]] || return 64
  count=0
  while read -r expected partition reader archive extra; do
    [[ -z "${expected:-}" || "$expected" == \#* ]] && continue
    [[ -z "${extra:-}" && "$expected" =~ ^[0-9a-f]{64}$ &&
       ( "$partition" == system || "$partition" == system_ext ) &&
       "$reader" == /* && "$archive" == /* && "$reader" != *..* &&
       "$archive" != *..* && "$reader" != *$'\t'* && "$archive" != *$'\t'* ]] || return 64
    case "$partition" in
      system)
        [[ "$reader" == /system/etc/compatconfig/*.xml &&
           "$archive" == /system/etc/compatconfig/*.xml ]] || return 64
        ;;
      system_ext)
        [[ "$reader" == /etc/compatconfig/*.xml &&
           "$archive" == /system/system_ext/etc/compatconfig/*.xml ]] || return 64
        ;;
    esac
    actual="$input$archive"
    [[ -f "$actual" && ! -L "$actual" ]] || return 69
    [[ "$(shasum -a 256 "$actual")" == "$expected  $actual" ]] || return 65
    count=$((count + 1))
  done < "$manifest"
  [[ "$count" == 11 ]] || return 64
  local base path name expected_names actual_names
  local -a temporary=()
  cleanup() { rm -f -- "${temporary[@]}"; }
  trap cleanup EXIT
  for base in "$input/system" "$input/system/etc" \
      "$input/system/etc/compatconfig" "$input/system/system_ext" \
      "$input/system/system_ext/etc" "$input/system/system_ext/etc/compatconfig"; do
    [[ -d "$base" && ! -L "$base" ]] || return 69
  done
  for base in "$input/system/etc/compatconfig" "$input/system/system_ext/etc/compatconfig"; do
    [[ -d "$base" && ! -L "$base" ]] || return 69
    expected_names="$(mktemp "${TMPDIR:-/tmp}/compat-expected.XXXXXX")"
    actual_names="$(mktemp "${TMPDIR:-/tmp}/compat-actual.XXXXXX")"
    temporary+=("$expected_names" "$actual_names")
    while read -r expected partition reader archive extra; do
      [[ -z "${expected:-}" || "$expected" == \#* ]] && continue
      if [[ "$base" == "$input/system/etc/compatconfig" &&
            "$archive" == /system/etc/compatconfig/* ]] ||
         [[ "$base" == "$input/system/system_ext/etc/compatconfig" &&
            "$archive" == /system/system_ext/etc/compatconfig/* ]]; then
        printf '%s\n' "${archive##*/}" >> "$expected_names"
      fi
    done < "$manifest"
    for path in "$base"/* "$base"/.[!.]* "$base"/..?*; do
      [[ -e "$path" || -L "$path" ]] || continue
      name="${path##*/}"
      printf '%s\n' "$name" >> "$actual_names"
    done
    sort -u "$expected_names" -o "$expected_names"
    sort -u "$actual_names" -o "$actual_names"
    cmp "$expected_names" "$actual_names" || return 65
  done
)

darwin_art_prepare_system_compat_config_artifact() (
  set -euo pipefail
  local root image output
  root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
  output="${DARWIN_ART_ANDROID16_COMPAT_CONFIG_ROOT:-$root/_build/android16-system-compat-config}"
  [[ "$output" = /* && "$output" != / && ! -L "$output" ]] || return 64
  if [[ -e "$output" ]]; then
    darwin_art_verify_system_compat_config_inventory "$output" \
      "$root/upstream/android16-system-compat-files.lock" || return
    echo "$output"
    return 0
  fi
  image="${DARWIN_ART_ANDROID16_SYSTEM_IMAGE:-$HOME/Library/Android/sdk/system-images/android-36/google_apis_playstore_ps16k/arm64-v8a/system.img}"
  [[ -f "$image" && ! -L "$image" ]] || return 64
  mkdir -p "${output%/*}"
  bash "$root/tools/prepare-android16-compat-config.sh" "$image" "$output" >&2
  darwin_art_verify_system_compat_config_inventory "$output" \
    "$root/upstream/android16-system-compat-files.lock" || return
  echo "$output"
)
