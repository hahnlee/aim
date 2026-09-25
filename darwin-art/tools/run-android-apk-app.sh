#!/usr/bin/env bash
set -euo pipefail
# Bash 3 (the macOS system shell) treats expansion of an initialized empty
# array as unset under nounset. This launcher intentionally supports base-only
# APKs, so keep strict error/pipe handling while relaxing only nounset.
set +u

root="$(cd "$(dirname "$0")/.." && pwd)"
source "$root/tools/lib/system-private-data.sh"
source "$root/tools/lib/runtime-system-image.sh"
source "$root/tools/lib/system-service-environment.sh"
source "$root/tools/lib/runtime-system-service.sh"
source "$root/tools/lib/app-launch-arguments.sh"
source "$root/tools/lib/chromium-launch-arguments.sh"
installed_record=""
split_apks=()
if [[ "${1:-}" == "--record" ]]; then
  installed_record="${2:-}"
  seconds="${3:-86400}"
  [[ -f "$installed_record" ]] || {
    echo "installed launch record does not exist: $installed_record" >&2
    exit 66
  }
  [[ "$(sed -n '1p' "$installed_record")" == "darwin-art-launch-v1" ]] || {
    echo "installed launch record version is unsupported" >&2
    exit 65
  }
  apk="$(sed -n 's/^apk=//p' "$installed_record")"
  app_dex="$(sed -n 's/^dex=//p' "$installed_record")"
  metadata="$(sed -n 's/^metadata=//p' "$installed_record")"
  while IFS= read -r split_apk; do
    [[ -z "$split_apk" ]] || split_apks+=("$split_apk")
  done < <(sed -n 's/^split=//p' "$installed_record")
else
  apk="${1:-}"
  if [[ "${2:-}" == "--split" ]]; then
    shift
    while [[ "${1:-}" == "--split" ]]; do
      [[ -n "${2:-}" ]] || {
        echo "--split requires an APK path" >&2
        exit 64
      }
      split_apks+=("$2")
      shift 2
    done
    seconds="${1:-86400}"
  elif [[ "${2:-}" == *.apk ]]; then
    # Convenience form for the common original base.apk plus one ABI split.
    split_apks+=("$2")
    seconds="${3:-86400}"
  else
    seconds="${2:-86400}"
  fi
fi
[[ -n "$apk" ]] || {
  echo "usage: $0 BASE_APK [VISIBLE_SECONDS] | $0 BASE_APK --split ABI_APK [VISIBLE_SECONDS] | --record RECORD [VISIBLE_SECONDS]" >&2
  exit 64
}
source_apk="$(cd "$(dirname "$apk")" && pwd)/$(basename "$apk")"
[[ -f "$source_apk" ]] || {
  echo "APK does not exist: $source_apk" >&2
  exit 66
}
normalized_split_apks=()
if [[ ${#split_apks[@]} -gt 0 ]]; then
for split_apk in "${split_apks[@]}"; do
  split_apk="$(cd "$(dirname "$split_apk")" && pwd)/$(basename "$split_apk")"
  [[ -f "$split_apk" ]] || {
    echo "split APK does not exist: $split_apk" >&2
    exit 66
  }
  [[ "$split_apk" != *:* ]] || {
    echo "split APK path cannot contain ':' (splitSourceDirs uses ':' delimiter): $split_apk" >&2
    exit 64
  }
  normalized_split_apks+=("$split_apk")
done
fi
if [[ ${#normalized_split_apks[@]} -gt 0 ]]; then
  split_apks=("${normalized_split_apks[@]}")
else
  split_apks=()
fi
[[ "$seconds" =~ ^([0-9]+)(\.[0-9]+)?$ ]] || {
  echo "VISIBLE_SECONDS must be a non-negative number" >&2
  exit 64
}

if [[ -z "$installed_record" ]]; then
  app_dex="$source_apk"
  external_dex="${source_apk%.apk}.dex"
  if ! unzip -Z1 "$source_apk" | grep -Fx 'classes.dex' >/dev/null; then
    [[ -f "$external_dex" ]] || {
      echo "preoptimized APK requires its deoptimized DEX sidecar: $external_dex" >&2
      exit 69
    }
    app_dex="$external_dex"
  fi
  metadata_tool="$root/target/release/android-apk-app-runtime"
  metadata_arguments=("$source_apk")
  [[ "$app_dex" == "$source_apk" ]] || metadata_arguments+=("$app_dex")
  if [[ ${#split_apks[@]} -gt 0 ]]; then
    for split_apk in "${split_apks[@]}"; do
      metadata_arguments+=(--split "$split_apk")
    done
  fi
  if [[ "$app_dex" == "$source_apk" ]]; then
    if [[ -x "$metadata_tool" ]]; then
      metadata="$("$metadata_tool" "${metadata_arguments[@]}")"
    else
      metadata="$(cargo run -q --release \
        --manifest-path "$root/tools/android-apk-app-runtime/Cargo.toml" -- \
        "${metadata_arguments[@]}")"
    fi
  else
    if [[ -x "$metadata_tool" ]]; then
      metadata="$("$metadata_tool" "${metadata_arguments[@]}")"
    else
      metadata="$(cargo run -q --release \
        --manifest-path "$root/tools/android-apk-app-runtime/Cargo.toml" -- \
        "${metadata_arguments[@]}")"
    fi
  fi
fi
metadata_refreshed=0
manifest_schema="$(sed -n 's/^apk-app-runtime: .* manifest_schema=\([^ ]*\) .*/\1/p' <<<"$metadata")"
manifest_schema_needs_refresh=0
if [[ -n "$installed_record" ]]; then
  # Installed records carry an immutable metadata snapshot. Schema 4 adds the
  # service permission/exported/enabled projection required by system services;
  # schema 6 adds each Activity's own screenOrientation, configChanges and
  # hardware acceleration; schema 7 adds the application icon resource;
  # schema 8 adds MAIN/INFO activities for launch-intent resolution.
  case "$manifest_schema" in
    ""|0|1|2|3|4|5|6|7) manifest_schema_needs_refresh=1 ;;
    8) ;;
    *)
      echo "installed package metadata schema is unsupported: $manifest_schema" >&2
      exit 69
      ;;
  esac
fi
if [[ "$manifest_schema_needs_refresh" == "1" ]]; then
  # Installed launch records are immutable package payload references, but
  # their PackageManager projection is versioned runtime state. Re-inspect the
  # unchanged installed APK when that projection schema advances, just as
  # Android rebuilds package settings from package manifests after an OTA.
  metadata_tool="$root/target/release/android-apk-app-runtime"
  [[ -x "$metadata_tool" ]] || {
    echo "installed package metadata is stale; rebuild the runtime before launch" >&2
    exit 69
  }
  refresh_arguments=("$source_apk")
  [[ "$app_dex" == "$source_apk" ]] || refresh_arguments+=("$app_dex")
  for split_apk in "${split_apks[@]}"; do
    refresh_arguments+=(--split "$split_apk")
  done
  refreshed_metadata="$($metadata_tool "${refresh_arguments[@]}")"
  refreshed_schema="$(sed -n 's/^apk-app-runtime: .* manifest_schema=\([^ ]*\) .*/\1/p' \
    <<<"$refreshed_metadata")"
  [[ "$refreshed_schema" == "7" ]] || {
    echo "rebuilt APK metadata tool does not support manifest schema 7" >&2
    exit 69
  }
  metadata="$refreshed_metadata"
  metadata_refreshed=1
fi
package="$(sed -n 's/^apk-app-runtime: package=\([^ ]*\) .*/\1/p' <<<"$metadata")"
application="$(sed -n 's/^apk-app-runtime: .* application=\([^ ]*\) .*/\1/p' <<<"$metadata")"
activity="$(sed -n 's/^apk-app-runtime: .* activity=\([^ ]*\) .*/\1/p' <<<"$metadata")"
launch_component="$(sed -n 's/^apk-app-runtime: .* launch_component=\([^ ]*\) .*/\1/p' <<<"$metadata")"
screen_orientation="$(sed -n 's/^apk-app-runtime: .* screen_orientation=\([^ ]*\) .*/\1/p' <<<"$metadata")"
if [[ -z "$screen_orientation" ]]; then
  # v1 launch records predate orientation metadata. Re-inspect the source APK
  # when the already-built metadata tool is available; otherwise preserve the
  # old unspecified-orientation behavior instead of rejecting the record.
  screen_orientation=-1
  metadata_tool="$root/target/release/android-apk-app-runtime"
  if [[ -x "$metadata_tool" ]]; then
    refresh_arguments=("$source_apk")
    [[ "$app_dex" == "$source_apk" ]] || refresh_arguments+=("$app_dex")
    for split_apk in "${split_apks[@]}"; do
      refresh_arguments+=(--split "$split_apk")
    done
    if [[ "$app_dex" == "$source_apk" ]]; then
      if refreshed_metadata="$($metadata_tool "${refresh_arguments[@]}")"; then
        :
      else
        refreshed_metadata=""
      fi
    else
      if refreshed_metadata="$($metadata_tool "${refresh_arguments[@]}")"; then
        :
      else
        refreshed_metadata=""
      fi
    fi
    refreshed_orientation="$(sed -n 's/^apk-app-runtime: .* screen_orientation=\([^ ]*\) .*/\1/p' <<<"$refreshed_metadata")"
    [[ -z "$refreshed_orientation" ]] || screen_orientation="$refreshed_orientation"
  fi
fi
descriptor="$(sed -n 's/^apk-app-runtime: .* descriptor=\([^ ]*\) .*/\1/p' <<<"$metadata")"
activities="$(sed -n 's/^apk-app-runtime: .* activities=\([^ ]*\) .*/\1/p' <<<"$metadata")"
activity_aliases="$(sed -n 's/^apk-app-runtime: .* activity_aliases=\([^ ]*\) .*/\1/p' <<<"$metadata")"
services="$(sed -n 's/^apk-app-runtime: .* services=\([^ ]*\) .*/\1/p' <<<"$metadata")"
receivers="$(sed -n 's/^apk-app-runtime: .* receivers=\([^ ]*\) .*/\1/p' <<<"$metadata")"
service_metadata="$(sed -n 's/^apk-app-runtime: .* service_metadata=\([^ ]*\) .*/\1/p' <<<"$metadata")"
application_metadata="$(sed -n 's/^apk-app-runtime: .* application_metadata=\([^ ]*\) .*/\1/p' <<<"$metadata")"
providers="$(sed -n 's/^apk-app-runtime: .* providers=\([^ ]*\) application_metadata=.*/\1/p' <<<"$metadata")"
version_code="$(sed -n 's/^apk-app-runtime: .* version_code=\([^ ]*\) .*/\1/p' <<<"$metadata")"
version_name="$(sed -n 's/^apk-app-runtime: .* version_name=\([^ ]*\) .*/\1/p' <<<"$metadata")"
theme="$(sed -n 's/^apk-app-runtime: .* theme=\([^ ]*\) .*/\1/p' <<<"$metadata")"
target_sdk="$(sed -n 's/^apk-app-runtime: .* target_sdk=\([^ ]*\) .*/\1/p' <<<"$metadata")"
requested_permissions="$(sed -n 's/^apk-app-runtime: .* permissions=\([^ ]*\) .*/\1/p' <<<"$metadata")"
debuggable="$(sed -n 's/^apk-app-runtime: .* debuggable=\([^ ]*\) .*/\1/p' <<<"$metadata")"
hardware_accelerated="$(sed -n 's/^apk-app-runtime: .* hardware_accelerated=\([^ ]*\) .*/\1/p' <<<"$metadata")"
activity_hardware_accelerated="$(sed -n 's/^apk-app-runtime: .* activity_hardware_accelerated=\([^ ]*\) .*/\1/p' <<<"$metadata")"
label="$(sed -n 's/^apk-app-runtime: .* label=\(.*\) label_res=.*/\1/p' <<<"$metadata")"
label_res="$(sed -n 's/^apk-app-runtime: .* label_res=\([^ ]*\) .*/\1/p' <<<"$metadata")"
activity_label="$(sed -n 's/^apk-app-runtime-activity-label: label=\(.*\) label_res=.*/\1/p' <<<"$metadata")"
activity_label_res="$(sed -n 's/^apk-app-runtime-activity-label: .* label_res=\([^ ]*\)$/\1/p' <<<"$metadata")"
application_icon_res="$(sed -n 's/^apk-app-runtime-application-icon: res=\([^ ]*\)$/\1/p' <<<"$metadata")"
icon="$(sed -n 's/^apk-app-runtime: .* icon=\([^ ]*\) .*/\1/p' <<<"$metadata")"
native_count="$(sed -n 's/^apk-app-runtime: .* native=\([^ ]*\) .*/\1/p' <<<"$metadata")"
native_root="$(sed -n 's/^apk-app-runtime: .* native_root=\([^ ]*\)$/\1/p' <<<"$metadata")"
[[ -n "$package" && -n "$application" && -n "$activity" && -n "$launch_component" && -n "$screen_orientation" && -n "$descriptor" && -n "$activities" && -n "$activity_aliases" && -n "$services" && -n "$receivers" && -n "$service_metadata" && -n "$application_metadata" && -n "$requested_permissions" && -n "$version_code" && -n "$theme" && -n "$target_sdk" && -n "$debuggable" && -n "$hardware_accelerated" && -n "$activity_hardware_accelerated" && -n "$label" && -n "$label_res" && -n "$icon" && -n "$native_count" && -n "$native_root" ]] || {
  echo "could not decode inspected APK metadata" >&2
  exit 65
}
if [[ -n "${DARWIN_ART_APK_ACTIVITY_OVERRIDE:-}" ]]; then
  requested_activity="$DARWIN_ART_APK_ACTIVITY_OVERRIDE"
  requested_entry="$(tr ',' '\n' <<<"$activities" | \
    sed -n "s#^${requested_activity}=##p" | head -1)"
  [[ -n "$requested_entry" ]] || {
    echo "requested Activity is not declared by the APK: $requested_activity" >&2
    exit 65
  }
  activity="$requested_activity"
  launch_component="$requested_activity"
  descriptor="L$(tr '.' '/' <<<"$activity");"
  theme="$requested_entry"
fi

runtime_abi="darwin-art-darwin-native-v1"
profile_mount=""
if [[ ! -x "$root/target/release/darwin-artctl" ||
      ! -x "$root/target/release/darwin-artd" ]]; then
  cargo build -q --release -p darwin-art-profile --bins
fi
profile_ctl="$root/target/release/darwin-artctl"
profile_mount="$("$profile_ctl" ensure)"
export DARWIN_ART_PROFILE_CTL="$profile_ctl"
export DARWIN_ART_PROFILE_SOCKET
DARWIN_ART_PROFILE_SOCKET="$("$profile_ctl" socket)"
if [[ "$metadata_refreshed" == "1" ]]; then
  # Publish the regenerated PackageManager projection through the profile
  # authority before system_server resolves the service declaration. Preserve
  # every identity/path/app-id field from the existing launch record.
  record_stage="$(mktemp "$profile_mount/run/launch-record.XXXXXX")"
  while IFS= read -r record_line; do
    [[ "$record_line" == metadata=* ]] || printf '%s\n' "$record_line"
  done <"$installed_record" >"$record_stage"
  printf 'metadata=%s\n' "$metadata" >>"$record_stage"
  "$profile_ctl" register "$package" "$record_stage"
  rm -f "$record_stage"
fi
# App-data isolation changes only the package sandbox. Android system services
# remain profile-scoped and common to every APK, just as they are on a device.
# Darwin's sockaddr_un.sun_path is only 104 bytes, so keep process-control
# endpoints in a short private runtime directory.
system_socket_dir="/tmp/darwin-art-$(id -u)"
mkdir -p "$system_socket_dir"
chmod 0700 "$system_socket_dir"
profile_socket_id="$(printf '%s' "$profile_mount" | shasum -a 256 | awk '{print substr($1, 1, 16)}')"
export DARWIN_ART_SYSTEM_SERVER_SOCKET="$system_socket_dir/$profile_socket_id.system.sock"
export DARWIN_ART_SURFACEFLINGER_SOCKET="$system_socket_dir/$profile_socket_id.sf.sock"
if [[ -n "${DARWIN_ART_APK_INSTALL_ROOT:-}" ]]; then
  install_root="$DARWIN_ART_APK_INSTALL_ROOT"
elif [[ -n "$profile_mount" ]]; then
  install_root="$profile_mount/packages"
else
  install_root="$root/_build/installed-apps"
fi
native_cache_root="${DARWIN_ART_NATIVE_CACHE_ROOT:-$root/_build/native-artifact-cache}"
native_converter="${DARWIN_ART_NATIVE_CONVERTER:-none}"
installer="$root/target/release/darwin-art-apk-install"
native_resolver="$root/target/release/darwin-art-native-resolve"
if [[ -z "$installed_record" ]]; then
  if [[ ! -x "$installer" || ! -x "$native_resolver" ]]; then
    cargo build -q --release -p darwin-art-apk-install
    cargo build -q --release -p darwin-art-native-artifact --bin darwin-art-native-resolve
  fi
  extractor="none"
  if [[ "$native_count" != "0" ]]; then
    extractor="$root/target/release/android-apk-native-extract"
    if [[ ! -x "$extractor" ]]; then
      cargo build -q --release \
        --manifest-path "$root/tools/android-apk-native-extract/Cargo.toml"
    fi
  fi
  install_arguments=("$source_apk" "$install_root" "$package" "$version_code"
    "$native_root" "$extractor" "$runtime_abi" "$native_cache_root"
    "$native_converter")
  for split_apk in "${split_apks[@]}"; do
    install_arguments+=("$split_apk")
  done
  install_output="$("$installer" "${install_arguments[@]}")"
  apk_sha256="$(sed -n 's/^apk-install: .* apk_sha256=\([^ ]*\) .*/\1/p' \
    <<<"$install_output")"
elif [[ -n "$installed_record" ]]; then
  apk_sha256="$(sed -n 's/^sha256=//p' "$installed_record")"
  install_output="apk-install: cached package=$package apk_sha256=$apk_sha256"
fi
[[ "$apk_sha256" =~ ^[0-9a-f]{64}$ ]] || {
  echo "could not decode installed APK identity" >&2
  exit 65
}
if [[ -z "$installed_record" ]]; then
  installed_directory="$install_root/$package/$version_code/$apk_sha256"
  apk="$installed_directory/base.apk"
else
  installed_directory="$(dirname "$apk")"
  # Launch records may predate the installer migration that provisions the
  # writable Android oat cache. Reuse the installer's permission-safe helper
  # so ART can publish anonymous vdex without opening the APK payload.
  if [[ ! -x "$installer" ]]; then
    cargo build -q --release -p darwin-art-apk-install
  fi
  "$installer" --ensure-oat "$installed_directory" >/dev/null
fi
installed_split_apks=()
if [[ -z "$installed_record" ]]; then
  for index in "${!split_apks[@]}"; do
    installed_split_apks+=("$installed_directory/split-$index.apk")
  done
else
  installed_split_apks=("${split_apks[@]}")
fi
[[ -f "$apk" ]] || {
  echo "installed APK is missing: $apk" >&2
  exit 69
}
if [[ -z "$installed_record" && "$app_dex" == "$source_apk" ]]; then
  app_dex="$apk"
fi

if [[ -z "$installed_record" && -n "$profile_mount" ]]; then
  if [[ "$app_dex" != "$apk" ]]; then
    code_directory="$profile_mount/system/package-code/$package/$apk_sha256"
    mkdir -p "$code_directory"
    persistent_dex="$code_directory/classes.dex"
    if [[ ! -f "$persistent_dex" ]]; then
      dex_stage="$code_directory/.classes.dex.$$.stage"
      cp "$app_dex" "$dex_stage"
      chmod 0400 "$dex_stage"
      mv "$dex_stage" "$persistent_dex"
    fi
    app_dex="$persistent_dex"
  fi
  record_stage="$(mktemp "$profile_mount/run/launch-record.XXXXXX")"
  {
    printf 'darwin-art-launch-v1\n'
    printf 'apk=%s\n' "$apk"
    printf 'dex=%s\n' "$app_dex"
    printf 'sha256=%s\n' "$apk_sha256"
    printf 'native_library_dir=%s\n' "$installed_directory/android-elf/arm64-v8a"
    for split_apk in "${installed_split_apks[@]}"; do
      printf 'split=%s\n' "$split_apk"
    done
    printf 'metadata=%s\n' "$metadata"
  } >"$record_stage"
  "$profile_ctl" register "$package" "$record_stage"
  rm -f "$record_stage"
  if [[ "${DARWIN_ART_INSTALL_ONLY:-0}" == "1" ]]; then
    if [[ "${DARWIN_ART_PACKAGED_RUNTIME:-0}" == "1" ]]; then
      host="$root/target/release/darwin-art-host"
    else
      host="$root/target/debug/darwin-art-host"
    fi
    if [[ -n "${DARWIN_ART_HOST_BUNDLE:-}" ]]; then
      host="$DARWIN_ART_HOST_BUNDLE/Contents/MacOS/darwin-art-host"
    fi
    [[ -x "$host" ]] || {
      echo "darwin-art host is missing; run cargo xtask build before installing" >&2
      exit 69
    }
    if [[ "${DARWIN_ART_PACKAGED_RUNTIME:-0}" == "1" ]]; then
      "$root/tools/prepare-darwin-art-host.sh" "$host" packaged
    else
      "$root/tools/prepare-darwin-art-host.sh" "$host" development
    fi
    echo "$metadata"
    echo "$install_output"
    echo "darwin-art: installed package=$package"
    exit 0
  fi
else
  for runtime_binary in "$native_resolver"; do
    [[ -x "$runtime_binary" ]] || {
      echo "installed run requires a prebuilt runtime; run cargo xtask build" >&2
      exit 69
    }
  done
fi

if [[ "${DARWIN_ART_PACKAGED_RUNTIME:-0}" == "1" ]]; then
  host="$root/target/release/darwin-art-host"
else
  host="$root/target/debug/darwin-art-host"
fi
if [[ -n "${DARWIN_ART_HOST_BUNDLE:-}" ]]; then
  host="$DARWIN_ART_HOST_BUNDLE/Contents/MacOS/darwin-art-host"
elif [[ -x "$root/DarwinARTHost.app/Contents/MacOS/darwin-art-host" ]]; then
  host="$root/DarwinARTHost.app/Contents/MacOS/darwin-art-host"
fi
[[ -x "$host" ]] || {
  echo "darwin-art host is missing: $host" >&2
  exit 69
}
if [[ "${DARWIN_ART_PACKAGED_RUNTIME:-0}" == "1" ]]; then
  "$root/tools/prepare-darwin-art-host.sh" "$host" packaged
else
  "$root/tools/prepare-darwin-art-host.sh" "$host" development
fi
runtime="$root/_build/runtime-graphics-link-probe/libdarwin_art_runtime_graphics.dylib"
core_oj="${DARWIN_ART_CORE_OJ_JAR:-$root/_prebuilt/android-16/bootclasspath/core-oj.jar}"
if [[ -z "${DARWIN_ART_CORE_OJ_JAR:-}" && \
      -f "$root/_build/android16-core-oj-compat/core-oj-compat.jar" ]]; then
  core_oj="$root/_build/android16-core-oj-compat/core-oj-compat.jar"
fi
core_libart="$root/_prebuilt/android-16/bootclasspath/core-libart.jar"
framework="${DARWIN_ART_FRAMEWORK_JAR:-$root/_prebuilt/android-16/bootclasspath/framework.jar}"
if [[ -z "${DARWIN_ART_FRAMEWORK_JAR:-}" && \
      -f "$root/_build/android16-framework-compat/framework-compat.jar" ]]; then
  # The detached host has no DeviceConfig service manager.  Use the merged
  # framework boot image whose no-service DeviceConfig seam preserves AOSP
  # default-valued feature flags during widget construction.
  framework="$root/_build/android16-framework-compat/framework-compat.jar"
fi
framework_location="$root/_prebuilt/android-16/bootclasspath/framework-location.jar"
core_icu="$root/_build/bootclasspath/core-icu4j-api36.jar"
conscrypt="$root/_build/android16-ps16k-r07/extracted/conscrypt/javalib/conscrypt.jar"
framework_bluetooth="$root/_build/android16-ps16k-r07/extracted/bt/javalib/framework-bluetooth.jar"
framework_mediaprovider="$root/_build/android16-ps16k-r07/extracted/mediaprovider/javalib/framework-mediaprovider.jar"
framework_permission="$root/_build/android16-ps16k-r07/extracted/permission/javalib/framework-permission.jar"
framework_permission_s="$root/_build/android16-ps16k-r07/extracted/permission/javalib/framework-permission-s.jar"
okhttp="$root/_build/android16-ps16k-r07/extracted/art/javalib/okhttp.jar"
# Match Android 16's boot-class-path ordering: framework-location follows the
# core framework (and framework-graphics, once split out here) before APEX
# framework modules.  The host ABI accepts the remaining colon-separated
# components through its boot-tail field.
boot_tail="$framework_location:$conscrypt:$framework_bluetooth:$framework_mediaprovider:$framework_permission:$framework_permission_s:$okhttp:$core_icu"
# One image-derived classpath owns runtime resolution, including service children.
# The fixed positional ABI above remains until callers migrate; ART consumes
# this complete ordered path instead of treating the ABI slots as class order.
export DARWIN_ART_BOOT_CLASSPATH
DARWIN_ART_BOOT_CLASSPATH="$(python3 "$root/tools/bootclasspath/resolve.py")"
export DARWIN_ART_BOOT_CLASSPATH_LOCATIONS
DARWIN_ART_BOOT_CLASSPATH_LOCATIONS="$(python3 "$root/tools/bootclasspath/resolve.py" --locations)"
support_dex="$root/_build/runtime-support-dex/dex/classes.dex"
# Apps and newly started system services use the same immutable Android image.
# Writable /data and /storage are separate authorities, never image contents.
image_mode=development
[[ "${DARWIN_ART_PACKAGED_RUNTIME:-0}" != "1" ]] || image_mode=packaged
shared_image_store="${DARWIN_ART_SYSTEM_IMAGE_STORE:-$HOME/Library/Application Support/DarwinART/runtime-images}"
system_archive="$(darwin_art_runtime_system_archive "$root" "$image_mode")"
system_root="$(darwin_art_prepare_runtime_system_image "$root" "$host" "$image_mode" "$shared_image_store")"
fonts_xml="$system_root/system/etc/fonts.xml"
roboto="$system_root/system/fonts/Roboto-Regular.ttf"
framework_res="$system_root/system/framework/framework-res.apk"
if [[ "$image_mode" == development ]]; then
  cargo run -q --manifest-path "$root/Cargo.toml" -p art-bootstrap -- build-runtime-support-dex-incremental >/dev/null
elif [[ ! -f "$support_dex" ]]; then
    echo "installed run requires prebuilt support DEX; run cargo xtask build" >&2
    exit 69
fi
if [[ -n "$profile_mount" ]]; then
  # The production support DEX is also loaded by the profile's system server. ART
  # may create oat/vdex beside any DEX it opens, so never point a packaged
  # launch at the read-only signed Manager bundle. Keep a profile-owned copy
  # with the same bytes and let dexopt place artifacts under this writable
  # cache root.
  support_dex_cache="$profile_mount/system/dex-cache/runtime-support-dex"
  mkdir -p "$support_dex_cache"
  # Existing app/system-server processes can have this DEX mmap'ed. Publish a
  # new inode atomically instead of truncating their live mapping (or racing
  # another launch's chmod/copy). Identical launches need no publication.
  if ! cmp -s "$support_dex" "$support_dex_cache/classes.dex"; then
    support_dex_stage="$(mktemp "$support_dex_cache/classes.dex.XXXXXX")"
    cp "$support_dex" "$support_dex_stage"
    chmod 0400 "$support_dex_stage"
    mv -f "$support_dex_stage" "$support_dex_cache/classes.dex"
  fi
  support_dex="$support_dex_cache/classes.dex"
fi
export DARWIN_ART_RUNTIME_HOST_FILES="$DARWIN_ART_BOOT_CLASSPATH:$support_dex:$app_dex"
for input in "$host" "$runtime" "$core_oj" "$core_libart" "$framework" "$framework_location" "$core_icu" "$conscrypt" "$framework_bluetooth" "$framework_mediaprovider" "$framework_permission" "$framework_permission_s" "$okhttp" "$support_dex" "$fonts_xml" "$roboto" "$framework_res"; do
  [[ -f "$input" ]] || {
    echo "runtime input is missing: $input" >&2
    echo "run the bootstrap/graphics build gates first" >&2
    exit 69
  }
done

# Only the launch-owned icon is temporary. The image outlives all app and
# service processes; shell exit must neither chmod/delete it nor kill children
# to reclaim it. Process termination belongs to the profile supervisor.
icon_file=""
cleanup_launch_icon() {
  [[ -z "$icon_file" ]] || rm -f -- "$icon_file"
}
trap cleanup_launch_icon EXIT

icu_runtime="$root/_build/icu-runtime-adapters/runtime"
export ANDROID_I18N_ROOT="$icu_runtime/i18n"
export ANDROID_DATA="$icu_runtime/data"
export ANDROID_TZDATA_ROOT="$icu_runtime/tzdata"
export DARWIN_ART_APK_APP_PACKAGE="$package"
# The launcher grants exactly this Activity host one macOS desktop target.
# system_server and daemon-created service/renderer children explicitly strip
# the capability while retaining their Android graphics/IOSurface paths.
export DARWIN_ART_DESKTOP_PRESENTATION=1
export DARWIN_ART_APK_APP_APPLICATION="$application"
export DARWIN_ART_APK_APP_ACTIVITY="$activity"
export DARWIN_ART_APK_APP_LAUNCH_COMPONENT="$launch_component"
export DARWIN_ART_APK_APP_SCREEN_ORIENTATION="$screen_orientation"
export DARWIN_ART_APK_APP_DESCRIPTOR="$descriptor"
export DARWIN_ART_APK_APP_ACTIVITIES="$activities"
export DARWIN_ART_APK_APP_ACTIVITY_ALIASES="$activity_aliases"
export DARWIN_ART_APK_APP_SERVICES="$services"
export DARWIN_ART_APK_APP_RECEIVERS="$receivers"
export DARWIN_ART_APK_APP_SERVICE_METADATA="$service_metadata"
export DARWIN_ART_APK_APP_PROVIDERS="$providers"
export DARWIN_ART_APK_APP_METADATA="$application_metadata"
export DARWIN_ART_APK_APP_VERSION_CODE="$version_code"
export DARWIN_ART_APK_APP_VERSION_NAME="$version_name"
export DARWIN_ART_APK_APP_THEME="$theme"
export DARWIN_ART_APK_APP_TARGET_SDK="$target_sdk"
export DARWIN_ART_APK_APP_REQUESTED_PERMISSIONS="$requested_permissions"
export DARWIN_ART_RUNTIME_TARGET_SDK_VERSION="$target_sdk"
export DARWIN_ART_RUNTIME_JAVA_DEBUGGABLE="$debuggable"
export DARWIN_ART_APK_APP_APK_SHA256="$apk_sha256"
export DARWIN_ART_NATIVE_RUNTIME_ABI="$runtime_abi"
export DARWIN_ART_APK_APP_LABEL="$label"
export DARWIN_ART_APK_APP_LABEL_RES="$label_res"
export DARWIN_ART_APK_ACTIVITY_LABEL="$activity_label"
export DARWIN_ART_APK_ACTIVITY_LABEL_RES="${activity_label_res:-0}"
export DARWIN_ART_APK_APP_ICON_RES="${application_icon_res:-0}"
if [[ -n "${DARWIN_ART_APP_DATA_ROOT:-}" ]]; then
  app_data_root="$DARWIN_ART_APP_DATA_ROOT"
else
  app_data_root="$profile_mount/data/apps"
fi
app_data_dir="$app_data_root/$package"
mkdir -p "$app_data_dir"
darwin_art_load_app_launch_arguments "$app_data_dir/launch-arguments"
private_data_root="$app_data_dir/private-data"
mkdir -p "$private_data_root"
chmod 0700 "$private_data_root"
export DARWIN_ART_ANDROID_PRIVATE_DATA_ROOT="$private_data_root"
export DARWIN_ART_APK_APP_DATA_DIR="$app_data_dir"
export DARWIN_ART_APK_APP_DATA_GUEST_DIR="/data/user/0/$package"

# ContextImpl creates these package-private directories before app code runs.
# The detached host exposes the same writable subtree inside the sealed guest
# root; Java and native code therefore agree on the Android /data path.
guest_app_data="$private_data_root/user/0/$package"
guest_device_data="$private_data_root/user_de/0/$package"
mkdir -p "$guest_app_data/files" "$guest_app_data/cache" \
  "$guest_app_data/code_cache" "$guest_app_data/no_backup" \
  "$guest_app_data/databases" "$guest_app_data/shared_prefs" \
  "$guest_device_data/files" "$guest_device_data/cache" \
  "$guest_device_data/code_cache" "$guest_device_data/no_backup" \
  "$guest_device_data/databases" "$guest_device_data/shared_prefs"
chmod 0500 "$private_data_root/user" "$private_data_root/user/0"
chmod 0500 "$private_data_root/user_de" "$private_data_root/user_de/0"
chmod 0700 "$guest_app_data" "$guest_app_data/files" \
  "$guest_app_data/cache" "$guest_app_data/code_cache" \
  "$guest_app_data/no_backup" "$guest_app_data/databases" \
  "$guest_app_data/shared_prefs" "$guest_device_data" \
  "$guest_device_data/files" "$guest_device_data/cache" \
  "$guest_device_data/code_cache" "$guest_device_data/no_backup" \
  "$guest_device_data/databases" "$guest_device_data/shared_prefs"

# Chromium reads its Android command line from an app-private file. Keep that
# transport delivers first-run switches and the required Graphite/Vulkan backend
# for normal launches as well as acceptance tests. Contradictory GPU switches
# are rejected; they must not silently select a fallback rendering path.
if [[ "$package" == "org.chromium.chrome" ]]; then
  export DARWIN_ART_APP_COMMAND_LINE_FILE="${DARWIN_ART_APP_COMMAND_LINE_FILE:-chrome-command-line}"
  chromium_command_line="${DARWIN_ART_APP_COMMAND_LINE:-}"
  chromium_command_line="$(darwin_art_configure_chromium_launch_arguments "$chromium_command_line")"
  export DARWIN_ART_APP_COMMAND_LINE="$chromium_command_line"
fi
if [[ -n "${DARWIN_ART_APP_COMMAND_LINE_FILE:-}" ]]; then
  [[ "$DARWIN_ART_APP_COMMAND_LINE_FILE" == "$(basename "$DARWIN_ART_APP_COMMAND_LINE_FILE")" ]] || {
    echo "app command-line filename must be a basename" >&2
    exit 64
  }
  [[ -n "${DARWIN_ART_APP_COMMAND_LINE:-}" ]] || {
    echo "DARWIN_ART_APP_COMMAND_LINE_FILE requires DARWIN_ART_APP_COMMAND_LINE" >&2
    exit 64
  }
  command_line_dir="$private_data_root/local"
  command_line_file="$command_line_dir/$DARWIN_ART_APP_COMMAND_LINE_FILE"
  command_line_debug_dir="$command_line_dir/tmp"
  command_line_debug_file="$command_line_debug_dir/$DARWIN_ART_APP_COMMAND_LINE_FILE"
  mkdir -p "$command_line_dir"
  chmod 0700 "$command_line_dir"
  mkdir -p "$command_line_debug_dir"
  chmod 0700 "$command_line_debug_dir"
  command_line_payload="$(tr '\n' ' ' <<<"$DARWIN_ART_APP_COMMAND_LINE")"
  [[ ! -e "$command_line_file" ]] || chmod 0600 "$command_line_file"
  [[ ! -e "$command_line_debug_file" ]] || chmod 0600 "$command_line_debug_file"
  printf '_ %s\n' "$command_line_payload" >"$command_line_file"
  printf '_ %s\n' "$command_line_payload" >"$command_line_debug_file"
  chmod 0400 "$command_line_file"
  chmod 0400 "$command_line_debug_file"
  chmod 0500 "$command_line_debug_dir"
  chmod 0500 "$command_line_dir"
fi
chmod 0500 "$private_data_root"

# Persistent profile storage is mounted by the process filesystem owner.
# Migration preserves file identities and the old host-producer path, and
# refuses conflicting trees rather than merging or replacing user files.
shared_storage_root="$profile_mount/storage"
if [[ "$app_data_root" != "$profile_mount/data/apps" ]]; then
  # An explicitly isolated app-data root must not acquire the real profile's
  # external files. Keep its persistent storage on the same volume as its data.
  shared_storage_root="$app_data_root/.shared-storage"
  mkdir -p "$shared_storage_root"
  chmod 0700 "$shared_storage_root"
fi
external_storage_dir="$("$host" --prepare-external-storage \
  "$shared_storage_root" "$app_data_dir" "$package")"
export DARWIN_ART_ANDROID_SHARED_STORAGE_ROOT="$shared_storage_root"
export DARWIN_ART_APK_APP_EXTERNAL_DIR="/storage/emulated/0/Android/data/$package/files"
if [[ "$icon" != "none" ]]; then
  icon_file="$(mktemp "${TMPDIR:-/tmp}/darwin-art-apk-icon.XXXXXX")"
  unzip -p "$apk" "$icon" >"$icon_file"
  chmod 0400 "$icon_file"
  export DARWIN_ART_APK_APP_ICON="$icon_file"
else
  unset DARWIN_ART_APK_APP_ICON
fi
export DARWIN_ART_APK_APP_SUPPORT_DEX="$support_dex"
export DARWIN_ART_APK_APP_RESOURCE_APK="$apk"
split_source_dirs=""
for split_apk in "${installed_split_apks[@]}"; do
  if [[ -n "$split_source_dirs" ]]; then
    split_source_dirs="$split_source_dirs:$split_apk"
  else
    split_source_dirs="$split_apk"
  fi
done
export DARWIN_ART_APK_APP_SPLIT_SOURCE_DIRS="$split_source_dirs"
export DARWIN_ART_FRAMEWORK_RES_APK="$framework_res"
export DARWIN_ART_TEST_FONTS_XML="/system/etc/fonts.xml"
export DARWIN_ART_TEST_FONT="/system/fonts/Roboto-Regular.ttf"
# Minikin opens font files below the native graphics boundary rather than
# through the Java guest-filesystem facade. Keep the guest paths above for the
# Android contract, and grant the runtime's bootstrap seam these two explicit
# host capabilities so native font loading resolves the same immutable files.
export DARWIN_ART_HOST_FONTS_XML="$fonts_xml"
export DARWIN_ART_HOST_FONT="$roboto"
export DARWIN_ART_ANDROID_FILESYSTEM_ROOT="$system_root"
export DARWIN_ART_ANDROID_SYSTEM_ROOT="$system_root/system"
export DARWIN_ART_ANDROID_SYSTEM_NATIVE_DIR="$system_root/system/lib64"
# Retina remains the default for the desktop host, but callers may select the
# logical phone surface (scale 1) for Android configuration-sensitive tests.
export DARWIN_ART_WINDOW_SCALE="${DARWIN_ART_WINDOW_SCALE:-2}"

# A project-built ANGLE exposes Metal textures as EGLImages, which is required
# for Android AHardwareBuffer storage identity. Prefer it over the older ANGLE
# bundled with Android Studio; an explicit environment override remains first.
if [[ -z "${DARWIN_ART_ANGLE_DIRECTORY:-}" ]]; then
  angle_candidate="$root/_build/angle-source/out/DarwinArtRelease"
  if [[ -f "$angle_candidate/libEGL.dylib" &&
        -f "$angle_candidate/libGLESv2.dylib" ]]; then
    export DARWIN_ART_ANGLE_DIRECTORY="$angle_candidate"
  fi
fi
if [[ -z "${DARWIN_ART_ANGLE_DIRECTORY:-}" && -n "${ANDROID_HOME:-}" ]]; then
  angle_candidate="$ANDROID_HOME/emulator/lib64/gles_angle"
  if [[ -f "$angle_candidate/libEGL.dylib" &&
        -f "$angle_candidate/libGLESv2.dylib" ]]; then
    export DARWIN_ART_ANGLE_DIRECTORY="$angle_candidate"
  fi
fi

# Android P+ Chrome enables its direct-rendering display compositor and needs
# a thread-safe Graphite/Dawn backing. The packaged MoltenVK provider is the
# Vulkan ICD for that Android contract; the guest still sees libvulkan.so and
# never receives a Darwin dlopen handle. Keep this provider check scoped to
# Chrome's launch contract; other APKs retain their existing Vulkan behavior.
if [[ -z "${DARWIN_ART_MOLTENVK_DYLIB:-}" ]]; then
  moltenvk_candidate="$root/_build/moltenvk/libMoltenVK.dylib"
  if [[ -f "$moltenvk_candidate" ]]; then
    export DARWIN_ART_MOLTENVK_DYLIB="$moltenvk_candidate"
  fi
fi
if [[ "$package" == "org.chromium.chrome" ]]; then
  [[ "${DARWIN_ART_MOLTENVK_DYLIB:-}" == /* &&
     -f "${DARWIN_ART_MOLTENVK_DYLIB:-}" &&
     ! -L "${DARWIN_ART_MOLTENVK_DYLIB:-}" ]] || {
    echo "Chrome Vulkan launch requires an absolute regular MoltenVK provider" >&2
    exit 69
  }
fi

# Platform Conscrypt/libssl uses the Android LIBC_R unwind contract even for
# APKs that have no packaged native libraries. Prepare it independently of
# the APK native-count so Java-only apps get the same provider as native APKs.
unwind_provider="$root/_build/android-unwind-provider/libdarwin_art_android_unwind.so"
if [[ -n "$installed_record" && ! -f "$unwind_provider" ]]; then
  echo "installed run requires prebuilt Android unwind provider" >&2
  exit 69
elif [[ -z "$installed_record" ]]; then
  "$root/tools/build-android-unwind-provider.sh" "$unwind_provider" >/dev/null
fi
export DARWIN_ART_ANDROID_UNWIND_PROVIDER="$unwind_provider"

if [[ "$native_count" != "0" ]]; then
  [[ "$native_root" != "none" ]] || {
    echo "APK native metadata did not select an arm64 root library" >&2
    exit 65
  }
  native_directory="$installed_directory/android-elf/arm64-v8a"
  [[ -f "$native_directory/$native_root" ]] || {
    echo "installed APK native root is missing" >&2
    exit 69
  }
  export DARWIN_ART_APK_APP_NATIVE_PATH="$native_directory/$native_root"
  export DARWIN_ART_APK_APP_NATIVE_DIR="$native_directory"
  darwin_directory="$native_cache_root/$apk_sha256/$runtime_abi"
  native_resolution="$("$native_resolver" "$apk_sha256" "$runtime_abi" \
    "$native_directory" "$darwin_directory")"
  native_backend="$(sed -n 's/^native-resolve: PASS backend=\([^ ]*\) .*/\1/p' \
    <<<"$native_resolution")"
  case "$native_backend" in
    darwin)
      export DARWIN_ART_APK_NATIVE_BACKEND=darwin
      export DARWIN_ART_APK_DARWIN_DIRECTORY="$darwin_directory"
      ;;
    elf)
      export DARWIN_ART_APK_NATIVE_BACKEND=elf
      unset DARWIN_ART_APK_DARWIN_DIRECTORY
      ;;
    *)
      echo "native artifact resolver returned an invalid backend" >&2
      exit 65
      ;;
  esac
  export DARWIN_ART_APK_MANAGED_NATIVE_LOAD="${DARWIN_ART_APK_MANAGED_NATIVE_LOAD:-1}"
else
  unset DARWIN_ART_APK_APP_NATIVE_PATH
  unset DARWIN_ART_APK_APP_NATIVE_DIR
  unset DARWIN_ART_APK_MANAGED_NATIVE_LOAD
  unset DARWIN_ART_APK_NATIVE_BACKEND
  unset DARWIN_ART_APK_DARWIN_DIRECTORY
fi

echo "$metadata"
echo "$install_output"
[[ "$native_count" == "0" ]] || echo "$native_resolution"
# A separately signed development host permits late debugger attachment while
# retaining the normal profile exec path, ASLR, and initial signal behavior.
if [[ -n "${DARWIN_ART_DEBUG_HOST:-}" ]]; then
  [[ -x "$DARWIN_ART_DEBUG_HOST" ]] || { echo "debug host is not executable" >&2; exit 2; }
  host="$DARWIN_ART_DEBUG_HOST"
fi
# Keep the launcher shell alive for debugger modes as well. An exec'd lldb
# would bypass launch-owned icon cleanup just like an exec'd app host.
run_lldb() {
  lldb "$@"
  local status=$?
  exit "$status"
}
if [[ -n "${DARWIN_ART_LLDB_COMMAND_FILE:-}" ]]; then
  [[ -f "$DARWIN_ART_LLDB_COMMAND_FILE" ]] || { echo "debugger command file is missing" >&2; exit 2; }
  run_lldb --source "$DARWIN_ART_LLDB_COMMAND_FILE" -- "$host" --window-seconds "$seconds" \
    "$runtime" "$core_oj" "$core_libart" "$framework" "$boot_tail" "$app_dex"
fi
if [[ "${DARWIN_ART_LLDB:-0}" == "1" ]]; then
  run_lldb --batch \
    -o 'process handle SIGINFO --stop false --notify false --pass false' \
    -o run -k 'thread backtrace all -c 40' -k 'register read' -- "$host" --window-seconds "$seconds" \
    "$runtime" "$core_oj" "$core_libart" "$framework" "$boot_tail" "$app_dex"
fi
if [[ "${DARWIN_ART_LLDB:-0}" == "exit" ]]; then
  run_lldb --batch \
    -o 'breakpoint set -n exit' -o 'breakpoint set -n _exit' \
    -o 'breakpoint set -n pthread_exit' \
    -o 'breakpoint set -n darwin_art_bionic_exit' \
    -o 'breakpoint set -n darwin_art_bionic__exit' \
    -o run -k 'thread backtrace all -c 40' -k 'register read' -- "$host" --window-seconds "$seconds" \
    "$runtime" "$core_oj" "$core_libart" "$framework" "$boot_tail" "$app_dex"
fi
if [[ "${DARWIN_ART_LLDB:-0}" == "dex" ]]; then
  run_lldb --batch \
    -o 'breakpoint set -n _ZN3artL25DexFile_defineClassNativeEP7_JNIEnvP7_jclassP8_jstringP8_jobjectS7_S7_' \
    -o run -o 'register read x0 x1 x2 x3 x4 x5' \
    -o 'thread backtrace -c 30' -- "$host" --window-seconds "$seconds" \
    "$runtime" "$core_oj" "$core_libart" "$framework" "$boot_tail" "$app_dex"
fi
if [[ "${DARWIN_ART_LLDB:-0}" == "syscall-240" ]]; then
  run_lldb --batch \
    -o 'breakpoint set -n darwin_art_bionic_syscall_captured -c "*(unsigned long long*)$x0 == 240"' \
    -o run -o 'memory read -fx -s8 -c6 $x0' \
    -o 'thread backtrace -c 20' -- "$host" --window-seconds "$seconds" \
    "$runtime" "$core_oj" "$core_libart" "$framework" "$boot_tail" "$app_dex"
fi
if [[ "${DARWIN_ART_LLDB:-0}" == "fs-stat" ]]; then
  run_lldb --batch \
    -o 'breakpoint set -n darwin_art_libcore_stat -c "(int)strncmp((char*)$x0, \"/data/local\", 11) == 0"' \
    -o run -o 'memory read -s1 -c128 $x0' \
    -o 'thread backtrace -c 24' -- "$host" --window-seconds "$seconds" \
    "$runtime" "$core_oj" "$core_libart" "$framework" "$boot_tail" "$app_dex"
fi
host_command=("$host" --window-seconds "$seconds" \
  "$runtime" "$core_oj" "$core_libart" "$framework" "$boot_tail" "$app_dex")
if [[ -n "$profile_mount" ]]; then
  runtime_start="$(darwin_art_start_runtime_system_service \
    "$host" "$profile_mount" "$system_root" "$system_archive" "$shared_image_store" \
    "$framework_res" "$support_dex" "$runtime" "$core_oj" "$core_libart" "$framework" "$boot_tail" \
    )"
  darwin_art_apply_runtime_endpoints "$runtime_start"
  # The profile daemon must own the final host Child. A caller-owned
  # exec lease made the host's lifetime depend on this shell (and left the
  # manager/app shim with a second, unrelated owner). daemonize registers
  # the host PID before acknowledging it, so the shell can safely wait for
  # that exact process through the authoritative profile ps view.
  host_pid="$("$profile_ctl" daemonize "$package" "${host_command[@]}")"
  [[ "$host_pid" =~ ^[1-9][0-9]*$ ]] || {
    echo "darwin-artd returned an invalid application PID: $host_pid" >&2
    exit 69
  }

  # Manager/Finder shims use this explicit handoff mode: the shell performs
  # all package/runtime setup, transfers final-host ownership to darwin-artd,
  # and returns once that transfer is acknowledged. Ordinary invocations
  # remain synchronous and retain their requested window duration.
  if [[ "${DARWIN_ART_ASYNC_LAUNCH:-0}" == "1" ]]; then
    exit 0
  fi

  wait_for_daemon_process() {
    local pid="$1" expected_package="$2" processes
    while :; do
      # Do not use kill -0 or parent/child relationships: the profile daemon's
      # process registry is the ownership authority and is incarnation-safe.
      processes="$("$profile_ctl" ps)" || {
        echo "could not query darwin-artd process ownership" >&2
        return 69
      }
      if ! awk -F '\t' -v expected_pid="$pid" -v expected_package="$expected_package" \
          '$1 == expected_pid && $2 == expected_package { found = 1 }
           END { exit(found ? 0 : 1) }' <<<"$processes"; then
        return 0
      fi
      sleep 0.1
    done
  }
  wait_for_daemon_process "$host_pid" "$package"
  status=$?
  exit "$status"
fi
"${host_command[@]}"
status=$?
exit "$status"
