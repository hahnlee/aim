#!/bin/bash
# Archive an already-materialized AOSP build artifact, not a writable profile.
# Native and service-code extraction belong to their separate pinned producers.
darwin_art_package_system_root() (
  set -euo pipefail
  local input="$1" destination="$2" fonts="$3" framework="$4" required resource_stage helper_root services
  helper_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)" || return
  services="${5:-$helper_root/_build/android16-system-services/services.jar}"
  source "$helper_root/tools/lib/system-services-artifact.sh" || return
  darwin_art_verify_system_services "$services" || return
  [[ "$input" = /* && -d "$input" && ! -L "$input" && "$input" != / ]] || return 64
  [[ "$destination" = /* && ! -e "$destination" && ! -L "$destination" ]] || return 64
  [[ "$destination" != "$input/"* ]] || return 64
  [[ "$fonts" = /* && "$framework" = /* && -f "$framework" && ! -L "$framework" ]] || return 64
  source "$helper_root/tools/lib/system-native-inventory.sh" || return
  darwin_art_verify_system_native_inventory "$input" \
    "$helper_root/upstream/android16-system-native-files.lock" || return
  python3 "$helper_root/tools/bootclasspath/verify_fonts.py" "$fonts" || return
  unzip -tqq "$framework" || return
  unzip -p "$framework" AndroidManifest.xml >/dev/null || return
  unzip -p "$framework" resources.arsc >/dev/null || return
  [[ ! -e "$input/system/fonts" && ! -L "$input/system/fonts" &&
     ! -e "$input/system/framework" && ! -L "$input/system/framework" &&
     ! -e "$input/system/etc/fonts.xml" && ! -L "$input/system/etc/fonts.xml" &&
     ! -e "$input/system/etc/font_fallback.xml" && ! -L "$input/system/etc/font_fallback.xml" ]] || return 65
  for required in apex system linkerconfig; do
    [[ -d "$input/$required" && ! -L "$input/$required" ]] || return 69
  done
  for required in apex/apex-info-list.xml linkerconfig/ld.config.txt \
      linkerconfig/apex.libraries.config.txt \
      system/etc/public.libraries.txt system/etc/linker.config.pb \
      apex/com.android.art/apex_manifest.pb apex/com.android.runtime/apex_manifest.pb \
      apex/com.android.conscrypt/apex_manifest.pb apex/com.android.i18n/apex_manifest.pb \
      apex/com.android.tzdata/apex_manifest.pb; do
    [[ -f "$input/$required" && ! -L "$input/$required" ]] || {
      echo "incomplete Android system build artifact: $input/$required" >&2
      return 69
    }
  done
  # A shared runtime image must never accidentally package app/profile data.
  [[ ! -e "$input/data" && ! -L "$input/data" && ! -e "$input/storage" && ! -L "$input/storage" ]] || return 65
  mkdir -p "${destination%/*}" || return
  resource_stage="$(mktemp -d "${TMPDIR:-/tmp}/system-root-resources.XXXXXX")" || return
  trap 'rm -rf -- "$resource_stage"' EXIT
  mkdir -p "$resource_stage/boot" || return
  python3 "$helper_root/tools/bootclasspath/stage_system_root.py" \
    "$resource_stage/boot" > "$resource_stage/boot-entries" || return
  mkdir -p "$resource_stage/system/framework" || return
  cp -p "$framework" "$resource_stage/system/framework/framework-res.apk" || return
  cp -p "$services" "$resource_stage/system/framework/services.jar" || return
  darwin_art_verify_system_services "$resource_stage/system/framework/services.jar" || return
  # This directory is packaging structure, not a runtime-generated resource.
  # Give it a stable source timestamp so identical inputs produce identical
  # archive identities across builds instead of a new shared image each time.
  touch -r "$framework" "$resource_stage/system/framework" || return
  # Preserve Android absolute symlinks as archive entries: materializing them
  # inside a signed macOS bundle causes codesign to follow missing host paths.
  # No -h: never dereference those links or change upstream payload bytes.
  # Replace original native-image JAR entries with the exact selected ART
  # backing, once per Android path. Apply exclusions only to the native input,
  # not the subsequent append. Never publish two versions of the same file.
  /usr/bin/tar -cf "$destination" -X "$resource_stage/boot-entries" \
    -C "$input" apex system linkerconfig || return
  /usr/bin/tar -rf "$destination" \
    -C "$fonts" system/etc/fonts.xml system/etc/font_fallback.xml system/fonts \
    -C "$resource_stage" system/framework || return
  /usr/bin/tar -rf "$destination" -C "$resource_stage/boot" \
    -T "$resource_stage/boot-entries" || return
  chmod a-w "$destination" || return
)
