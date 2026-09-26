#!/bin/bash
# Archive an already-materialized AOSP build artifact, not a writable profile.
# Native and service-code extraction belong to their separate pinned producers.
darwin_art_package_system_root() (
  set -euo pipefail
  local input="$1" destination="$2" fonts="$3" framework="$4" required resource_stage helper_root services compat keychars
  helper_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)" || return
  services="${5:-$helper_root/_build/android16-system-services/services.jar}"
  source "$helper_root/tools/lib/system-services-artifact.sh" || return
  darwin_art_verify_system_services "$services" || return
  source "$helper_root/tools/lib/system-compat-artifact.sh" || return
  if [[ $# -ge 6 ]]; then
    compat="$6"
  else
    compat="$(darwin_art_prepare_system_compat_config_artifact)" || return
  fi
  darwin_art_verify_system_compat_config_inventory "$compat" \
    "$helper_root/upstream/android16-system-compat-files.lock" || return
  source "$helper_root/tools/lib/systemserver-classpath-artifact.sh" || return
  local server_jars
  server_jars="$(darwin_art_prepare_systemserver_classpath_artifact)" || return
  source "$helper_root/tools/lib/system-partition-artifact.sh" || return
  local system_partition
  system_partition="$(darwin_art_prepare_system_partition_artifact)" || return
  source "$helper_root/tools/lib/key-character-map-artifact.sh" || return
  if [[ $# -ge 7 ]]; then
    keychars="$7"
  else
    keychars="$(darwin_art_prepare_key_character_map_artifact)" || return
  fi
  darwin_art_verify_key_character_map_inventory "$keychars" \
    "$helper_root/upstream/android16-key-character-map.lock" || return
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
     ! -e "$input/system/etc/compatconfig" && ! -L "$input/system/etc/compatconfig" &&
     ! -e "$input/system/usr/keychars" && ! -L "$input/system/usr/keychars" &&
     ! -e "$input/system/system_ext" && ! -L "$input/system/system_ext" &&
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
  # system_server classpath JARs at their device paths. services.jar is the
  # separately verified artifact above; a JAR a native APEX payload already
  # carries must be the identical file.
  local expected kind path server_list
  server_list="$resource_stage/server-entries"
  : > "$server_list"
  mkdir -p "$resource_stage/server" || return
  while read -r expected kind path extra; do
    [[ -z "${expected:-}" || "$expected" == \#* || "$expected" == IMAGE_SHA256=* ]] && continue
    [[ "$path" == /system/framework/services.jar ]] && continue
    if [[ -e "$input$path" ]]; then
      [[ "$(shasum -a 256 "$input$path" | awk '{print $1}')" == "$expected" ]] || return 65
      continue
    fi
    mkdir -p "$resource_stage/server$(dirname "$path")" || return
    cp -p "$server_jars$path" "$resource_stage/server$path" || return
    chmod 0444 "$resource_stage/server$path" || return
    touch -r "$framework" "$resource_stage/server$path" || return
    printf '%s\n' "${path#/}" >> "$server_list"
  done < "$helper_root/upstream/android16-systemserverclasspath.lock"
  # derive_classpath's environment file (its /data/system/environ/classpath);
  # this image's classpaths are fixed when it is assembled.
  mkdir -p "$resource_stage/server/system/etc" || return
  {
    printf 'export SYSTEMSERVERCLASSPATH %s\n' "$(darwin_art_systemserver_classpath classpath)"
    printf 'export STANDALONE_SYSTEMSERVER_JARS %s\n' "$(darwin_art_systemserver_classpath standalone)"
  } > "$resource_stage/server/system/etc/classpath" || return
  chmod 0444 "$resource_stage/server/system/etc/classpath" || return
  touch -r "$framework" "$resource_stage/server/system/etc/classpath" || return
  printf '%s\n' system/etc/classpath >> "$server_list"
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
    -C "$compat" system/etc/compatconfig system/system_ext/etc/compatconfig || return
  /usr/bin/tar -rf "$destination" \
    -C "$keychars" system/usr/keychars || return
  /usr/bin/tar -rf "$destination" \
    -C "$fonts" system/etc/fonts.xml system/etc/font_fallback.xml system/fonts \
    -C "$resource_stage" system/framework || return
  /usr/bin/tar -rf "$destination" -C "$resource_stage/boot" \
    -T "$resource_stage/boot-entries" || return
  /usr/bin/tar -rf "$destination" -C "$resource_stage/server" \
    -T "$server_list" || return
  # System packages and platform configuration PackageManagerService scans.
  local partition_list="$resource_stage/partition-entries"
  # A JAR already packaged from the system_server classpath is the same file.
  darwin_art_system_partition_entries | grep -Fxv -f "$server_list" > "$partition_list" || return
  while read -r path; do
    [[ ! -e "$input/$path" && ! -L "$input/$path" ]] || {
      echo "system partition entry collides with the native image: $path" >&2
      return 65
    }
  done < "$partition_list"
  /usr/bin/tar -rf "$destination" -C "$system_partition" -T "$partition_list" || return
  # The host's own hardware feature declarations, beside the image's vendor
  # Vulkan XMLs in /vendor/etc/permissions.
  /usr/bin/tar -rf "$destination" -C "$helper_root/runtime/framework" \
    vendor/etc/permissions/darwin_host_hardware.xml || return
  # As on the device, /system_ext names the system_ext partition, which this
  # root keeps at /system/system_ext (DeviceProtos reads its flag proto there).
  mkdir -p "$resource_stage/root-links" || return
  ln -s system/system_ext "$resource_stage/root-links/system_ext" || return
  /usr/bin/tar -rf "$destination" -C "$resource_stage/root-links" system_ext || return
  # idmaps of the immutable framework overlays. AOSP's zygote runs
  # `idmap2 create-multiple` into /data/resource-cache on every boot; this
  # image is fixed, so they are created once here with the same policies
  # OverlayConfig requests (partition policy, overlayable enforced for
  # targetSdk >= Q) and read from /system/etc/resource-cache (ADR 0009).
  local idmap2="$helper_root/_build/android16-idmap2/idmap2" guest_root="$resource_stage/idmap-root"
  local cache="$resource_stage/idmap/system/etc/resource-cache" overlays=()
  [[ -x "$idmap2" ]] || bash "$helper_root/tools/build-android16-idmap2.sh" >&2 || return
  mkdir -p "$guest_root/system/framework" "$guest_root/product" "$cache" || return
  ln -s "$resource_stage/system/framework/framework-res.apk" \
    "$guest_root/system/framework/framework-res.apk" || return
  ln -s "$system_partition/product/overlay" "$guest_root/product/overlay" || return
  while read -r path; do
    [[ "$path" == product/overlay/*.apk ]] && overlays+=(--overlay-apk-path "/$path")
  done < "$partition_list"
  (( ${#overlays[@]} > 0 )) || return 65
  DARWIN_ART_IDMAP2_GUEST_ROOT="$guest_root" "$idmap2" create-multiple \
    --target-apk-path /system/framework/framework-res.apk "${overlays[@]}" \
    --policy public --policy product --idmap-dir "$cache" > "$resource_stage/idmaps" || return
  # idmap2 leaves out an overlay that overlays nothing it may (the emulator
  # characteristics RRO carries no resources), exactly as it would on device.
  [[ -s "$resource_stage/idmaps" ]] || return 65
  chmod 0444 "$cache"/* || return
  touch -r "$framework" "$cache"/* "$cache" || return
  /usr/bin/tar -rf "$destination" -C "$resource_stage/idmap" system/etc/resource-cache || return
  chmod a-w "$destination" || return
)
