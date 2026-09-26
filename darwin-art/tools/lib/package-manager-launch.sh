#!/bin/bash
# The host launcher's PackageManager client (ADR 0009). Installs are
# `cmd package install` in the profile's system server (a PackageInstaller
# session), and launch facts come from `launcher-info`, which asks
# PackageManager. Guest paths are mapped to the host trees that back them:
# /data/app is the profile's package store and the system server's /data is
# its private data root.

darwin_art_pm_system_data() {
  printf '%s\n' "$1/data/apps/android.system/private-data"
}

# darwin_art_pm_host_path MOUNT GUEST_PATH
darwin_art_pm_host_path() {
  local mount="$1" guest="$2"
  case "$guest" in
    /data/app/*) printf '%s\n' "$mount/packages/${guest#/data/app/}" ;;
    /data/*) printf '%s\n' "$(darwin_art_pm_system_data "$mount")/${guest#/data/}" ;;
    *) echo "no host path for guest path: $guest" >&2; return 65 ;;
  esac
}

# darwin_art_pm_install CTL MOUNT BASE_APK [SPLIT_APK...]; prints the package.
# The APKs are staged in the system server's /data/local/tmp, as adb does,
# and removed afterwards. An installed package with identical APKs is kept
# as it is.
darwin_art_pm_install() (
  set -euo pipefail
  local ctl="$1" mount="$2" base="$3"
  shift 3
  local tmp stage guest name index=0 info package installed paths=() same=1
  tmp="$(darwin_art_pm_system_data "$mount")/local/tmp"
  stage="$(mktemp -d "$tmp/darwin-art-install.XXXXXX")"
  trap 'rm -rf -- "$stage"' EXIT
  chmod 0755 "$stage"
  guest="/data/local/tmp/$(basename "$stage")"
  cp "$base" "$stage/base.apk"
  paths+=("$guest/base.apk")
  for name in "$@"; do
    cp "$name" "$stage/split-$index.apk"
    paths+=("$guest/split-$index.apk")
    index=$((index + 1))
  done
  chmod 0644 "$stage"/*.apk
  info="$("$ctl" archive-info "$guest/base.apk")" || {
    echo "not an installable APK: $base: $info" >&2
    return 65
  }
  package="$(sed -n 's/^package=//p' <<<"$info")"
  [[ -n "$package" ]] || { echo "APK declares no package: $base" >&2; return 65; }
  # `pm path`: the installed base and splits, in install order.
  if installed="$("$ctl" cmd package path "$package" 2>/dev/null)" &&
      [[ "$(grep -c '^package:' <<<"$installed")" == "${#paths[@]}" ]]; then
    local position=0 line
    while IFS= read -r line; do
      [[ "$line" == package:* ]] || continue
      local candidate="$stage/base.apk"
      (( position == 0 )) || candidate="$stage/split-$((position - 1)).apk"
      cmp -s "$candidate" "$(darwin_art_pm_host_path "$mount" "${line#package:}")" || same=0
      position=$((position + 1))
    done <<<"$installed"
  else
    same=0
  fi
  if [[ "$same" == 0 ]]; then
    local output
    output="$("$ctl" cmd package install -r "${paths[@]}" 2>&1)" || true
    grep -qx 'Success' <<<"$output" || {
      echo "PackageManager install of $package failed: $output" >&2
      return 70
    }
  fi
  printf '%s\n' "$package"
)

# darwin_art_pm_launcher_info CTL MOUNT PACKAGE: sets pm_uid, pm_activity,
# pm_source_dir, pm_split_source_dirs, pm_native_library_dir, pm_target_sdk,
# pm_debuggable, pm_version_code, pm_label and pm_icon (host paths).
darwin_art_pm_launcher_info() {
  local ctl="$1" mount="$2" package="$3" info line key value split mapped
  info="$("$ctl" launcher-info "$package")" || {
    echo "PackageManager has no launchable package $package: $info" >&2
    return 69
  }
  pm_uid="" pm_activity="" pm_source_dir="" pm_split_source_dirs=""
  pm_native_library_dir="" pm_target_sdk="" pm_debuggable="" pm_version_code=""
  pm_label="" pm_icon=""
  while IFS= read -r line; do
    key="${line%%=*}"
    value="${line#*=}"
    case "$key" in
      uid) pm_uid="$value" ;;
      activity) pm_activity="$value" ;;
      sourceDir) pm_source_dir="$(darwin_art_pm_host_path "$mount" "$value")" || return ;;
      splitSourceDirs)
        mapped=""
        if [[ -n "$value" ]]; then
          local splits
          IFS=: read -r -a splits <<<"$value"
          for split in "${splits[@]}"; do
            split="$(darwin_art_pm_host_path "$mount" "$split")" || return
            mapped="${mapped:+$mapped:}$split"
          done
        fi
        pm_split_source_dirs="$mapped"
        ;;
      nativeLibraryDir) pm_native_library_dir="$(darwin_art_pm_host_path "$mount" "$value")" || return ;;
      targetSdk) pm_target_sdk="$value" ;;
      debuggable) pm_debuggable="$value" ;;
      versionCode) pm_version_code="$value" ;;
      label) pm_label="$value" ;;
      icon) pm_icon="$(darwin_art_pm_host_path "$mount" "$value")" || return ;;
    esac
  done <<<"$info"
  [[ "$pm_uid" =~ ^[0-9]+$ && -n "$pm_activity" && -f "$pm_source_dir" &&
     "$pm_target_sdk" =~ ^[0-9]+$ && "$pm_debuggable" =~ ^[01]$ && -f "$pm_icon" ]] || {
    echo "incomplete PackageManager launch facts for $package" >&2
    return 65
  }
}
